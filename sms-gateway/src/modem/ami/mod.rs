//! Asterisk Manager Interface (AMI) client.
//!
//! Wire format: `Key: Value\r\n` lines, packets terminated by a blank line
//! (`\r\n\r\n`). Actions correlate to Responses via `ActionID`. Events are
//! identified by an `Event:` header.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use log::{debug, info, warn};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::sync::{broadcast, mpsc, oneshot, Mutex};
use tokio::time::sleep;

/// A parsed AMI packet (Response, Event, or unsolicited message).
#[derive(Debug, Clone)]
pub struct AmiPacket {
    pub fields: HashMap<String, String>,
}

impl AmiPacket {
    pub fn get(&self, key: &str) -> Option<&str> {
        self.fields.get(&key.to_ascii_lowercase()).map(|s| s.as_str())
    }

    pub fn event_name(&self) -> Option<&str> {
        self.get("event")
    }

    pub fn action_id(&self) -> Option<&str> {
        self.get("actionid")
    }

    pub fn response(&self) -> Option<&str> {
        self.get("response")
    }

    pub fn is_success(&self) -> bool {
        matches!(self.response(), Some(r) if r.eq_ignore_ascii_case("Success"))
    }

    pub fn message(&self) -> Option<&str> {
        self.get("message")
    }
}

/// Configuration for one AMI endpoint.
#[derive(Debug, Clone)]
pub struct AmiConfig {
    pub label: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub secret: String,
}

/// Pending action awaiting its response packet.
type PendingMap = Arc<Mutex<HashMap<String, oneshot::Sender<AmiPacket>>>>;

/// Per-connection write side (taken when we hold the connection lock).
type WriterHandle = Arc<Mutex<Option<OwnedWriteHalf>>>;

#[derive(Clone)]
pub struct AmiClient {
    cfg: AmiConfig,
    writer: WriterHandle,
    pending: PendingMap,
    event_tx: broadcast::Sender<AmiPacket>,
    /// Monotonic counter for ActionID generation.
    next_id: Arc<std::sync::atomic::AtomicU64>,
}

impl AmiClient {
    /// Spawn the connect/reconnect supervisor and return a handle.
    /// `shutdown` is a cancellation channel; when it fires, the supervisor exits.
    pub fn spawn(cfg: AmiConfig, mut shutdown: mpsc::Receiver<()>) -> Self {
        let (event_tx, _) = broadcast::channel(256);
        let writer: WriterHandle = Arc::new(Mutex::new(None));
        let pending: PendingMap = Arc::new(Mutex::new(HashMap::new()));
        let client = AmiClient {
            cfg: cfg.clone(),
            writer: writer.clone(),
            pending: pending.clone(),
            event_tx: event_tx.clone(),
            next_id: Arc::new(std::sync::atomic::AtomicU64::new(1)),
        };

        let cfg2 = cfg;
        let writer2 = writer;
        let pending2 = pending;
        let event_tx2 = event_tx;

        tokio::spawn(async move {
            let mut backoff = Duration::from_secs(1);
            const MAX_BACKOFF: Duration = Duration::from_secs(30);
            loop {
                tokio::select! {
                    biased;
                    _ = shutdown.recv() => {
                        info!("[ami {}] supervisor shutting down", cfg2.label);
                        return;
                    }
                    res = Self::run_once(&cfg2, &writer2, &pending2, &event_tx2) => {
                        match res {
                            Ok(()) => {
                                warn!("[ami {}] connection closed cleanly; reconnecting", cfg2.label);
                                backoff = Duration::from_secs(1);
                            }
                            Err(e) => {
                                warn!("[ami {}] connection error: {e}", cfg2.label);
                            }
                        }
                    }
                }

                // Drain pending callers with a synthetic failure so they don't hang
                {
                    let mut pend = pending2.lock().await;
                    pend.clear();
                }

                tokio::select! {
                    _ = shutdown.recv() => {
                        info!("[ami {}] supervisor shutting down during backoff", cfg2.label);
                        return;
                    }
                    _ = sleep(backoff) => {}
                }
                backoff = std::cmp::min(backoff * 2, MAX_BACKOFF);
            }
        });

        client
    }

    /// Subscribe to the event broadcast.
    pub fn subscribe(&self) -> broadcast::Receiver<AmiPacket> {
        self.event_tx.subscribe()
    }

    pub fn label(&self) -> &str {
        &self.cfg.label
    }

    /// One connection lifecycle: connect, login, run read loop until error.
    async fn run_once(
        cfg: &AmiConfig,
        writer_slot: &WriterHandle,
        pending: &PendingMap,
        event_tx: &broadcast::Sender<AmiPacket>,
    ) -> Result<()> {
        let addr = format!("{}:{}", cfg.host, cfg.port);
        info!("[ami {}] connecting to {}", cfg.label, addr);
        let stream = TcpStream::connect(&addr)
            .await
            .with_context(|| format!("connect to {addr}"))?;
        stream.set_nodelay(true).ok();
        let (read_half, mut write_half) = stream.into_split();

        // Read banner: a single line like "Asterisk Call Manager/9.0.0\r\n"
        let mut reader = BufReader::new(read_half);
        let mut banner = String::new();
        reader
            .read_line(&mut banner)
            .await
            .context("read AMI banner")?;
        if banner.is_empty() {
            return Err(anyhow!("AMI banner EOF before any data"));
        }
        debug!("[ami {}] banner: {}", cfg.label, banner.trim());

        // Inline login (we own write_half exclusively here, no pending yet).
        let login_id = format!("login-{}", cfg.label);
        let mut buf = String::new();
        buf.push_str("Action: Login\r\n");
        buf.push_str(&format!("ActionID: {login_id}\r\n"));
        buf.push_str(&format!("Username: {}\r\n", cfg.username));
        buf.push_str(&format!("Secret: {}\r\n", cfg.secret));
        buf.push_str("Events: on\r\n\r\n");
        write_half
            .write_all(buf.as_bytes())
            .await
            .context("write Login")?;
        write_half.flush().await.ok();

        // Read packets until we see the Login response.
        loop {
            let pkt = read_packet(&mut reader).await?;
            if pkt.action_id().is_some_and(|a| a == login_id) {
                if !pkt.is_success() {
                    return Err(anyhow!(
                        "AMI login rejected: {}",
                        pkt.message().unwrap_or("(no message)")
                    ));
                }
                info!("[ami {}] logged in", cfg.label);
                break;
            }
            // Stray FullyBooted etc before login completes — drop.
        }

        // Publish writer handle so callers can send actions.
        {
            let mut slot = writer_slot.lock().await;
            *slot = Some(write_half);
        }

        // Read loop until EOF/error.
        let read_result = read_loop(&mut reader, pending, event_tx, &cfg.label).await;

        // Drop writer so callers immediately fail.
        {
            let mut slot = writer_slot.lock().await;
            *slot = None;
        }

        read_result
    }

    /// Send an Action and await its first Response packet (correlated by ActionID).
    pub async fn action(&self, fields: Vec<(&str, &str)>) -> Result<AmiPacket> {
        self.action_with_timeout(fields, Duration::from_secs(10)).await
    }

    pub async fn action_with_timeout(
        &self,
        fields: Vec<(&str, &str)>,
        timeout: Duration,
    ) -> Result<AmiPacket> {
        let id = self
            .next_id
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let action_id = format!("a{}", id);

        let mut buf = String::new();
        let mut have_action = false;
        for (k, v) in &fields {
            if k.eq_ignore_ascii_case("Action") {
                have_action = true;
            }
            if k.eq_ignore_ascii_case("ActionID") {
                return Err(anyhow!("ActionID is managed internally"));
            }
            buf.push_str(k);
            buf.push_str(": ");
            buf.push_str(v);
            buf.push_str("\r\n");
        }
        if !have_action {
            return Err(anyhow!("missing Action: header"));
        }
        buf.push_str(&format!("ActionID: {action_id}\r\n\r\n"));

        let (tx, rx) = oneshot::channel();
        {
            let mut pend = self.pending.lock().await;
            pend.insert(action_id.clone(), tx);
        }

        // Hold the writer lock only long enough to write.
        {
            let mut slot = self.writer.lock().await;
            let w = slot
                .as_mut()
                .ok_or_else(|| anyhow!("AMI not connected"))?;
            w.write_all(buf.as_bytes()).await.context("write action")?;
            w.flush().await.ok();
        }

        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(pkt)) => Ok(pkt),
            Ok(Err(_)) => Err(anyhow!("AMI connection dropped before response")),
            Err(_) => {
                self.pending.lock().await.remove(&action_id);
                Err(anyhow!("AMI action timed out after {:?}", timeout))
            }
        }
    }
}

/// Read one packet (block of `Key: Value\r\n` lines terminated by `\r\n\r\n`).
async fn read_packet(reader: &mut BufReader<OwnedReadHalf>) -> Result<AmiPacket> {
    let mut fields: HashMap<String, String> = HashMap::new();
    let mut line = String::new();
    loop {
        line.clear();
        let n = reader.read_line(&mut line).await.context("read line")?;
        if n == 0 {
            return Err(anyhow!("AMI EOF"));
        }
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            return Ok(AmiPacket { fields });
        }
        if let Some(idx) = trimmed.find(':') {
            let (k, v) = trimmed.split_at(idx);
            let key = k.trim().to_ascii_lowercase();
            let val = v[1..].trim().to_string();
            fields.insert(key, val);
        }
        // Lines without ':' are ignored (e.g. trailing diagnostic text).
    }
}

async fn read_loop(
    reader: &mut BufReader<OwnedReadHalf>,
    pending: &PendingMap,
    event_tx: &broadcast::Sender<AmiPacket>,
    label: &str,
) -> Result<()> {
    loop {
        let pkt = read_packet(reader).await?;
        if let Some(aid) = pkt.action_id().map(str::to_owned) {
            let waiter = { pending.lock().await.remove(&aid) };
            if let Some(tx) = waiter {
                let _ = tx.send(pkt);
                continue;
            }
        }
        if pkt.event_name().is_some() {
            // ignore receiver count; consumers may not be subscribed
            let _ = event_tx.send(pkt);
        } else {
            debug!("[ami {label}] unhandled packet: {:?}", pkt.fields);
        }
    }
}
