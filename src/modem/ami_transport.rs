//! `Transport` implementation that drives one asterisk container via AMI.
//!
//! Maps high-level operations to AMI actions and AMI events to `ModemEvent`s
//! that the rest of the app already consumes.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use log::{debug, info, warn};
use tokio::sync::{mpsc, Mutex};
use uuid::Uuid;

use super::ami::{AmiClient, AmiConfig, AmiPacket};
use super::transport::{ModemEvent, SimInfo, Transport};
use super::types::SmsType;
use crate::db::ModemSMS;

/// Where MixMonitor writes WAV files inside the asterisk container.
/// (Host path is `/home/ht/docker/logs/<N>/recordings/...` via volume mount.)
const RECORDING_DIR: &str = "/logs/recordings";

/// PJSIP endpoint/registration name configured in `config/<N>/asterisk/pjsip.conf`.
const VOLTE_ENDPOINT: &str = "volte_ims";

#[derive(Debug, Clone)]
pub struct AmiTransportConfig {
    /// Stable sim id (ICCID once known, otherwise a fallback like `instance_3`).
    pub sim_id: String,
    /// Pre-known SIM identity, populated from config when available.
    pub sim_info: SimInfo,
    /// AMI connection config.
    pub ami: AmiConfig,
}

pub struct AmiTransport {
    cfg: AmiTransportConfig,
    client: AmiClient,
    /// Optional handle to the supervisor shutdown channel.
    _shutdown_tx: mpsc::Sender<()>,
    /// Outbound calls we've originated and are awaiting a Channel for.
    /// Key = ChannelId we asked AMI to use; value = our internal call_id.
    pending_originates: Arc<Mutex<std::collections::HashMap<String, String>>>,
}

impl AmiTransport {
    /// Spawn the AMI client supervisor and an event-pump task that forwards
    /// translated events to `events_tx`.
    pub fn spawn(cfg: AmiTransportConfig, events_tx: mpsc::Sender<ModemEvent>) -> Self {
        let (sd_tx, sd_rx) = mpsc::channel::<()>(1);
        let client = AmiClient::spawn(cfg.ami.clone(), sd_rx);
        let pending_originates = Arc::new(Mutex::new(std::collections::HashMap::new()));

        // Event pump: AMI events -> ModemEvent
        let mut rx = client.subscribe();
        let sim_id = cfg.sim_id.clone();
        let label = cfg.ami.label.clone();
        let pending = pending_originates.clone();
        tokio::spawn(async move {
            loop {
                match rx.recv().await {
                    Ok(pkt) => {
                        log::trace!(
                            "[ami {label}] event pkt: event={:?} fields={:?}",
                            pkt.event_name(),
                            pkt.fields
                        );
                        if let Some(ev) =
                            translate_event(&pkt, &sim_id, &pending).await
                        {
                            if events_tx.send(ev).await.is_err() {
                                info!("[ami {label}] event consumer dropped, stopping pump");
                                return;
                            }
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        warn!("[ami {label}] event channel lagged by {n} packets");
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        debug!("[ami {label}] event broadcast closed");
                        return;
                    }
                }
            }
        });

        Self {
            cfg,
            client,
            _shutdown_tx: sd_tx,
            pending_originates,
        }
    }
}

#[async_trait]
impl Transport for AmiTransport {
    fn sim_id(&self) -> &str {
        &self.cfg.sim_id
    }

    fn label(&self) -> &str {
        self.client.label()
    }

    async fn sim_info(&self) -> Result<SimInfo> {
        Ok(self.cfg.sim_info.clone())
    }

    async fn send_sms(&self, to: &str, body: &str) -> Result<()> {
        // res_pjsip_messaging MessageSend.
        //   To: pjsip:volte_ims/+<num>@volte_ims
        //   From: <endpoint name>
        //   Body: <text>
        let to_uri = format!("pjsip:{VOLTE_ENDPOINT}/{to}@{VOLTE_ENDPOINT}");
        let resp = self
            .client
            .action(vec![
                ("Action", "MessageSend"),
                ("To", &to_uri),
                ("From", VOLTE_ENDPOINT),
                ("Body", body),
            ])
            .await
            .context("MessageSend action")?;
        if !resp.is_success() {
            return Err(anyhow!(
                "MessageSend failed: {}",
                resp.message().unwrap_or("(no message)")
            ));
        }
        Ok(())
    }

    async fn originate_call(&self, to: &str) -> Result<String> {
        // Generate a per-call internal id and matching recording file.
        let call_id = Uuid::new_v4().to_string();
        let recording = format!("{RECORDING_DIR}/{call_id}_{to}.wav");
        let channel = format!("PJSIP/{to}@{VOLTE_ENDPOINT}");
        // We use Application=MixMonitor so the asterisk bridge starts recording
        // immediately on answer; ',b' = start when bridged.
        let mix_arg = format!("{recording},b");
        let resp = self
            .client
            .action(vec![
                ("Action", "Originate"),
                ("Channel", &channel),
                ("Application", "MixMonitor"),
                ("Data", &mix_arg),
                ("CallerID", VOLTE_ENDPOINT),
                ("Async", "true"),
            ])
            .await
            .context("Originate action")?;
        if !resp.is_success() {
            return Err(anyhow!(
                "Originate failed: {}",
                resp.message().unwrap_or("(no message)")
            ));
        }
        // Map any inbound Newchannel we don't yet know about to this call_id
        // via the recording path (which appears in MixMonitorStart event).
        self.pending_originates
            .lock()
            .await
            .insert(recording.clone(), call_id.clone());
        Ok(call_id)
    }

    async fn answer_call(&self) -> Result<()> {
        // Auto-answer is handled by the dialplan (Answer() + MixMonitor()).
        // Manual answer would require AMI Redirect to a dialplan extension;
        // not used in the current product flow.
        Ok(())
    }

    async fn hangup_call(&self, call_id: Option<&str>) -> Result<()> {
        let Some(channel) = call_id else {
            return Err(anyhow!("hangup_call requires a channel id for AMI transport"));
        };
        let _ = tokio::time::timeout(
            Duration::from_secs(5),
            self.client.action(vec![
                ("Action", "Hangup"),
                ("Channel", channel),
            ]),
        )
        .await
        .map_err(|_| anyhow!("Hangup timed out"))??;
        Ok(())
    }

    async fn read_sms(&self, _: SmsType) -> Result<Vec<ModemSMS>> {
        // AMI delivers SMS via the MessageReceived event; nothing to drain.
        Ok(vec![])
    }
}

/// Translate one AMI event packet into a ModemEvent, when relevant.
async fn translate_event(
    pkt: &AmiPacket,
    sim_id: &str,
    pending_originates: &Arc<Mutex<std::collections::HashMap<String, String>>>,
) -> Option<ModemEvent> {
    let name = pkt.event_name()?;
    match name {
        // Custom UserEvent emitted by the asterisk dialplan.
        // UserEvent: SmsReceived | CallStarted | CallAnswered | CallEnded
        // Fields differ by subtype (see asterisk-docker/.../extensions.conf).
        "UserEvent" => translate_user_event(pkt, sim_id),
        "MessageReceived" => {
            let from = pkt.get("from").unwrap_or("").to_string();
            let body = pkt.get("body").unwrap_or("").to_string();
            if from.is_empty() && body.is_empty() {
                return None;
            }
            Some(ModemEvent::SmsReceived {
                sim_id: sim_id.to_string(),
                from,
                body,
                timestamp: chrono::Utc::now(),
            })
        }
        // Inbound call ringing
        "Newchannel" => {
            let state = pkt.get("channelstatedesc").unwrap_or("");
            if !state.eq_ignore_ascii_case("Ringing") {
                return None;
            }
            let channel = pkt.get("channel")?.to_string();
            let phone = pkt.get("calleridnum").unwrap_or("").to_string();
            Some(ModemEvent::CallRinging {
                sim_id: sim_id.to_string(),
                call_id: channel,
                phone,
            })
        }
        // Call answered (state moved to Up)
        "Newstate" => {
            let state = pkt.get("channelstatedesc").unwrap_or("");
            if !state.eq_ignore_ascii_case("Up") {
                return None;
            }
            let channel = pkt.get("channel")?.to_string();
            Some(ModemEvent::CallAnswered {
                sim_id: sim_id.to_string(),
                call_id: channel,
            })
        }
        "Hangup" => {
            let channel = pkt.get("channel")?.to_string();
            Some(ModemEvent::CallEnded {
                sim_id: sim_id.to_string(),
                call_id: channel,
                recording_path: None,
            })
        }
        // MixMonitor finished writing a recording file.
        "MixMonitorStop" => {
            let file = pkt.get("file")?.to_string();
            let path = PathBuf::from(&file);
            // call_id: look up our internal id by the recording filename
            let call_id = pending_originates
                .lock()
                .await
                .remove(&file)
                .unwrap_or_else(|| file.clone());
            Some(ModemEvent::RecordingDone {
                sim_id: sim_id.to_string(),
                call_id,
                path,
            })
        }
        _ => None,
    }
}

/// Translate a `UserEvent: <subtype>` packet emitted by the dialplan.
/// Subtypes (set via dialplan `UserEvent(<subtype>,...)`):
///   SmsReceived    fields: From, Body
///   CallStarted    fields: Direction (inbound|outbound), Phone, CallId
///   CallAnswered   fields: CallId
///   CallEnded      fields: CallId, RecordingPath (optional)
/// Field names are lower-cased by the AMI parser.
fn translate_user_event(pkt: &AmiPacket, sim_id: &str) -> Option<ModemEvent> {
    let subtype = pkt.get("userevent")?;
    match subtype {
        "SmsReceived" => {
            let from = pkt.get("from").unwrap_or("").to_string();
            let body = pkt.get("body").unwrap_or("").to_string();
            if from.is_empty() && body.is_empty() {
                return None;
            }
            Some(ModemEvent::SmsReceived {
                sim_id: sim_id.to_string(),
                from,
                body,
                timestamp: chrono::Utc::now(),
            })
        }
        "CallStarted" => {
            let call_id = pkt.get("callid")?.to_string();
            let phone = pkt.get("phone").unwrap_or("").to_string();
            Some(ModemEvent::CallRinging {
                sim_id: sim_id.to_string(),
                call_id,
                phone,
            })
        }
        "CallAnswered" => {
            let call_id = pkt.get("callid")?.to_string();
            Some(ModemEvent::CallAnswered {
                sim_id: sim_id.to_string(),
                call_id,
            })
        }
        "CallEnded" => {
            let call_id = pkt.get("callid")?.to_string();
            let recording_path = pkt
                .get("recordingpath")
                .filter(|s| !s.is_empty())
                .map(PathBuf::from);
            Some(ModemEvent::CallEnded {
                sim_id: sim_id.to_string(),
                call_id,
                recording_path,
            })
        }
        _ => None,
    }
}
