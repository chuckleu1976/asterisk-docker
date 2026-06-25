//! AMI-only ModemManager.
//!
//! Owns one `Arc<dyn Transport>` per configured asterisk container, plus the
//! shared `ModemEvent` channel that all transports push into. The event
//! consumer started by `start_urc_handlers` fans events out to the SMS/Call
//! database, SSE, webhooks, and the whisper transcription pipeline.
//!
//! The legacy serial/AT path (`core.rs`, `pdu.rs`, `decode.rs`,
//! `recheck_fallback_modems`, CLCC polling) was removed in Phase B step 5.
//! Diagnostic getters now return `Ok(None)` by default; per-transport
//! implementations may override them.

use log::{error, info};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;

use crate::api::sse_manager::CallEvent;
use crate::api::SseManager;
use crate::config::{Settings, SmsStorage};
use crate::db::{Call, Contact, ModemSMS, SimCard};
use crate::webhook;

use super::types::{
    ModemInfo, NetworkRegistrationStatus, OperatorInfo, SmsType,
};

/// Configuration for local whisper.cpp speech-to-text transcription.
/// All three fields must be `Some` for transcription to be enabled.
#[derive(Debug, Clone)]
pub struct TranscribeConfig {
    pub ffmpeg_exe: String,
    pub whisper_exe: String,
    pub whisper_model: String,
    /// Language code: "en", "zh", "ja", etc., or "auto" for auto-detect.
    pub whisper_language: String,
    /// Delay before auto-answering incoming calls (seconds).
    pub auto_answer_delay_secs: u64,
    /// Whether to enable auto-answer for incoming calls.
    pub auto_answer_enabled: bool,
}

impl TranscribeConfig {
    pub fn from_settings(s: &Settings) -> Option<Arc<Self>> {
        match (&s.ffmpeg_exe, &s.whisper_exe, &s.whisper_model) {
            (Some(ffmpeg), Some(whisper), Some(model)) => Some(Arc::new(Self {
                ffmpeg_exe: ffmpeg.clone(),
                whisper_exe: whisper.clone(),
                whisper_model: model.clone(),
                whisper_language: s.whisper_language.clone().unwrap_or_else(|| "auto".to_string()),
                auto_answer_delay_secs: s.auto_answer_delay_secs.unwrap_or(2),
                auto_answer_enabled: s.auto_answer_enabled.unwrap_or(true),
            })),
            _ => None,
        }
    }
}

/// Lightweight summary returned by [`ModemManager::get_modem`] so the HTTP API
/// can still render a `com_port` / `baud_rate` pair for each modem. For AMI
/// transports `com_port` is `"ami:<host>:<port>"` and `baud_rate` is 0.
#[derive(Debug, Clone)]
pub struct ModemSummary {
    pub com_port: String,
    pub baud_rate: u32,
}

pub struct ModemManager {
    /// AMI-mode transports keyed by sim_id (ICCID, or fallback `instance_N`).
    transports: Arc<RwLock<HashMap<String, Arc<dyn super::Transport>>>>,
    /// Per-sim display address shown in the HTTP API (com_port + baud_rate).
    summaries: Arc<RwLock<HashMap<String, ModemSummary>>>,
    /// sim_id -> asterisk container instance number (1..=N). Used to map
    /// container-relative RecordingPath (`/logs/recordings/foo.wav`) to a host
    /// path under `{recordings_base_dir}/{instance}/recordings/foo.wav`.
    instance_by_sim: Arc<RwLock<HashMap<String, u8>>>,
    /// Host base dir for the per-instance `/logs/` volumes.
    recordings_base_dir: std::path::PathBuf,
    sim_cards_cache: Arc<RwLock<HashMap<String, SimCard>>>,
    /// Preserved for API parity with the old serial path; always empty now.
    pub unavailable_ports: Vec<(String, u32)>,
    /// Receiver end of the channel that transports push ModemEvents into.
    /// Wrapped in Mutex<Option<...>> so `start_urc_handlers` can take exclusive
    /// ownership.
    event_rx: tokio::sync::Mutex<Option<tokio::sync::mpsc::Receiver<super::ModemEvent>>>,
}

impl ModemManager {
    pub async fn initialize(config: &crate::config::AppConfig) -> anyhow::Result<Self> {
        // Shared mpsc channel that every AmiTransport pushes ModemEvent into.
        let (event_tx, event_rx) = tokio::sync::mpsc::channel::<super::ModemEvent>(1024);
        let mut transports: HashMap<String, Arc<dyn super::Transport>> = HashMap::new();
        let mut instance_by_sim: HashMap<String, u8> = HashMap::new();
        let mut summaries: HashMap<String, ModemSummary> = HashMap::new();

        for (index, device) in config.devices.iter().enumerate() {
            let instance = device.instance.unwrap_or((index + 1) as u8);
            let ami_host = device
                .ami_host
                .clone()
                .unwrap_or_else(|| "127.0.0.1".to_string());
            let ami_port = device.ami_port.unwrap_or(5037u16 + instance as u16);
            let ami_host_for_summary = ami_host.clone();
            let ami_user = device
                .ami_user
                .clone()
                .unwrap_or_else(|| "jolly".to_string());
            let secret = match &device.ami_secret_file {
                Some(path) => match std::fs::read_to_string(path) {
                    Ok(s) => s.trim().to_string(),
                    Err(e) => {
                        log::error!(
                            "Failed to read ami_secret_file {} for device {}: {}; skipping",
                            path, index, e
                        );
                        continue;
                    }
                },
                None => device
                    .ami_secret
                    .clone()
                    .unwrap_or_else(|| "geheim".to_string()),
            };

            // Stable sim_id: prefer configured ICCID; otherwise instance fallback.
            let sim_id = device
                .iccid
                .clone()
                .unwrap_or_else(|| format!("instance_{}", instance));

            let sim_info = super::SimInfo {
                iccid: device.iccid.clone(),
                imsi: device.imsi.clone(),
                msisdn: device.msisdn.clone(),
                mcc: None,
                mnc: None,
                sms_center: None,
            };

            let label = format!("asterisk{}@{}:{}", instance, ami_host, ami_port);
            let transport = super::ami_transport::AmiTransport::spawn(
                super::ami_transport::AmiTransportConfig {
                    sim_id: sim_id.clone(),
                    sim_info,
                    ami: super::ami::AmiConfig {
                        label,
                        host: ami_host,
                        port: ami_port,
                        username: ami_user,
                        secret,
                    },
                    reader_index: instance - 1,
                },
                event_tx.clone(),
            );
            transports.insert(sim_id.clone(), Arc::new(transport));
            instance_by_sim.insert(sim_id.clone(), instance);
            summaries.insert(
                sim_id.clone(),
                ModemSummary {
                    com_port: format!("asterisk{}", instance),
                    baud_rate: 0,
                },
            );
            info!(
                "Registered AMI transport for device {} (sim_id={})",
                index, sim_id
            );
        }
        // Drop the spare event_tx so the receiver knows when all transports go away.
        drop(event_tx);

        if transports.is_empty() {
            return Err(anyhow::anyhow!(
                "No AMI transports were configured; check [[devices]] in config.toml"
            ));
        }

        info!("Initialized {} AMI transport(s)", transports.len());

        let manager = Self {
            transports: Arc::new(RwLock::new(transports)),
            summaries: Arc::new(RwLock::new(summaries)),
            instance_by_sim: Arc::new(RwLock::new(instance_by_sim)),
            recordings_base_dir: config
                .settings
                .recordings_base_dir
                .clone()
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| std::path::PathBuf::from("/home/ht/docker/logs")),
            sim_cards_cache: Arc::new(RwLock::new(HashMap::new())),
            unavailable_ports: Vec::new(),
            event_rx: tokio::sync::Mutex::new(Some(event_rx)),
        };

        manager.init_sim_cache().await?;

        Ok(manager)
    }

    async fn init_sim_cache(&self) -> anyhow::Result<()> {
        let transports = self.transports.read().await;
        let sim_ids: Vec<&str> = transports.keys().map(|k| k.as_str()).collect();
        let sim_cards = SimCard::get_by_ids(&sim_ids).await?;
        let mut cache = self.sim_cards_cache.write().await;
        *cache = sim_cards;
        info!("Initialized SIM cache with {} cards", cache.len());
        Ok(())
    }

    // ─── Lookup ──────────────────────────────────────────────────────────────

    pub async fn get_sim_ids(&self) -> Vec<String> {
        self.transports.read().await.keys().cloned().collect()
    }

    /// Returns the AMI transport for this sim, if any.
    pub async fn get_transport(&self, sim_id: &str) -> Option<Arc<dyn super::Transport>> {
        self.transports.read().await.get(sim_id).cloned()
    }

    /// Compatibility wrapper for the HTTP API: returns a `ModemSummary` with
    /// the AMI host:port string in lieu of a serial `com_port`.
    pub async fn get_modem(&self, sim_id: &str) -> Option<ModemSummary> {
        self.summaries.read().await.get(sim_id).cloned()
    }

    /// Take ownership of the event receiver. Returns None if already taken.
    pub async fn take_event_rx(
        &self,
    ) -> Option<tokio::sync::mpsc::Receiver<super::ModemEvent>> {
        self.event_rx.lock().await.take()
    }

    pub async fn get_sim_card_cached(&self, sim_id: &str) -> Option<SimCard> {
        self.sim_cards_cache.read().await.get(sim_id).cloned()
    }

    /// Find the sim_id (ICCID) that matches a given SIM phone number.
    pub async fn find_sim_id_by_phone_number(&self, phone_number: &str) -> Option<String> {
        let cache = self.sim_cards_cache.read().await;
        cache.iter().find_map(|(sim_id, sim_card)| {
            sim_card.phone_number.as_deref().and_then(|p| {
                if p == phone_number {
                    Some(sim_id.clone())
                } else {
                    None
                }
            })
        })
    }

    pub async fn update_sim_cache(&self, sim_card: SimCard) {
        let mut cache = self.sim_cards_cache.write().await;
        cache.insert(sim_card.id.clone(), sim_card);
    }

    // ─── Operations (delegate to Transport) ──────────────────────────────────

    async fn transport_for(&self, sim_id: &str) -> anyhow::Result<Arc<dyn super::Transport>> {
        self.get_transport(sim_id)
            .await
            .ok_or_else(|| anyhow::anyhow!("No transport for sim_id {}", sim_id))
    }

    /// Send an SMS via AMI MessageSend and record it locally.
    /// Returns (sms_id, contact_id) for API parity with the legacy path.
    pub async fn send_sms(
        &self,
        sim_id: &str,
        contact: &Contact,
        message: &str,
    ) -> anyhow::Result<(i64, String)> {
        let t = self.transport_for(sim_id).await?;
        t.send_sms(&contact.name, message).await?;
        let sms = ModemSMS {
            contact: contact.name.clone(),
            timestamp: chrono::Utc::now().naive_utc(),
            message: message.to_string(),
            send: true,
            sim_id: sim_id.to_string(),
        };
        let sms_id = sms.insert().await?;
        Ok((sms_id, contact.id.clone()))
    }

    /// AMI transports do not poll for stored SMS — inbound arrives as events.
    pub async fn read_sms(
        &self,
        _sim_id: &str,
        _sms_type: SmsType,
    ) -> anyhow::Result<Vec<ModemSMS>> {
        Ok(Vec::new())
    }

    pub async fn read_sms_sync_insert(
        &self,
        _sim_id: &str,
        _sms_type: SmsType,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    /// Initiate an outbound call via AMI Originate, record it in the DB, and
    /// emit the SSE `outbound_call` event. The full call lifecycle (ringing →
    /// answered → ended + recording) lands on the UserEvent path.
    pub async fn make_call(
        &self,
        sim_id: &str,
        phone: &str,
        sse: Arc<SseManager>,
    ) -> anyhow::Result<String> {
        let t = self.transport_for(sim_id).await?;
        let call_id = t.originate_call(phone).await?;
        // DB insert and SSE event are handled by the dialplan's CallStarted
        // UserEvent → ModemEvent::CallRinging consumer.
        info!("[{}] outbound call {} to {} originated", sim_id, call_id, phone);
        Ok(call_id)
    }

    pub async fn answer_call(&self, sim_id: &str) -> anyhow::Result<()> {
        let t = self.transport_for(sim_id).await?;
        t.answer_call().await
    }

    /// Hang up the active call on this SIM. Returns `Ok(None)` because the AMI
    /// dialplan emits its own CallEnded UserEvent which is what marks the
    /// call ended in the DB; the API caller only needs to know the request
    /// was accepted.
    pub async fn hangup_call(
        &self,
        sim_id: &str,
    ) -> anyhow::Result<Option<(String, String)>> {
        let t = self.transport_for(sim_id).await?;
        t.hangup_call(None).await?;
        Ok(None)
    }

    // ─── Diagnostics (delegate to Transport; AMI returns None for most) ──────

    pub async fn check_network_registration(
        &self,
        sim_id: &str,
    ) -> anyhow::Result<Option<NetworkRegistrationStatus>> {
        self.transport_for(sim_id).await?.registration_status().await
    }

    pub async fn check_operator(
        &self,
        sim_id: &str,
    ) -> anyhow::Result<Option<OperatorInfo>> {
        self.transport_for(sim_id).await?.operator_info().await
    }

    pub async fn get_modem_model(
        &self,
        sim_id: &str,
    ) -> anyhow::Result<Option<ModemInfo>> {
        self.transport_for(sim_id).await?.modem_model().await
    }

    pub async fn get_imei(&self, sim_id: &str) -> anyhow::Result<Option<String>> {
        self.transport_for(sim_id).await?.imei().await
    }

    pub async fn get_sms_center(&self, sim_id: &str) -> anyhow::Result<Option<String>> {
        self.transport_for(sim_id).await?.sms_center().await
    }

    pub async fn get_network_info(
        &self,
        sim_id: &str,
    ) -> anyhow::Result<Option<String>> {
        self.transport_for(sim_id).await?.network_info().await
    }

    pub async fn get_sim_status(&self, sim_id: &str) -> anyhow::Result<Option<String>> {
        self.transport_for(sim_id).await?.sim_status().await
    }

    pub async fn get_memory_status(
        &self,
        sim_id: &str,
    ) -> anyhow::Result<Option<String>> {
        self.transport_for(sim_id).await?.memory_status().await
    }

    pub async fn get_temperature_info(
        &self,
        sim_id: &str,
    ) -> anyhow::Result<Option<String>> {
        self.transport_for(sim_id).await?.temperature_info().await
    }

    pub async fn set_sms_storage(
        &self,
        sim_id: &str,
        sms_storage: SmsStorage,
    ) -> anyhow::Result<()> {
        self.transport_for(sim_id).await?.set_sms_storage(sms_storage).await
    }

    pub async fn get_sms_storage_status(
        &self,
        sim_id: &str,
    ) -> anyhow::Result<Option<String>> {
        self.transport_for(sim_id).await?.sms_storage_status().await
    }

    // ─── Event handler ───────────────────────────────────────────────────────

    /// Spawn the AMI ModemEvent consumer. Each event is fanned out to the DB,
    /// SSE, webhooks, and (for CallEnded with a recording) the transcription
    /// pipeline. Called exactly once at startup.
    pub async fn start_urc_handlers(
        &self,
        sse_manager: Arc<SseManager>,
        webhook_manager: Option<webhook::WebhookManager>,
        transcribe_cfg: Option<Arc<TranscribeConfig>>,
    ) {
        let Some(mut rx) = self.take_event_rx().await else {
            log::warn!("start_urc_handlers called twice; event consumer already running");
            return;
        };
        let sse = sse_manager.clone();
        let wh = webhook_manager.clone();
        let cfg = transcribe_cfg.clone();
        let instance_by_sim = self.instance_by_sim.clone();
        let recordings_base = self.recordings_base_dir.clone();
        tokio::spawn(async move {
            info!("[ModemEvent] consumer started");
            while let Some(ev) = rx.recv().await {
                handle_modem_event(
                    ev,
                    &sse,
                    wh.as_ref(),
                    cfg.as_deref(),
                    &instance_by_sim,
                    &recordings_base,
                )
                .await;
            }
            info!("[ModemEvent] consumer stopped (channel closed)");
        });
    }
}

// ─── AMI ModemEvent fan-out ─────────────────────────────────────────────────

/// Extract a bare phone number from a SIP-ish "From" header.
/// Accepts forms like `<sip:+1234@host>`, `sip:+1234@host`, `"Alice" <sip:+1234>`,
/// or a bare `+1234`. Returns the input verbatim if no `sip:` URI was found.
fn extract_phone(from: &str) -> String {
    let s = from.trim();
    if s.is_empty() {
        return String::new();
    }
    let after_sip = match s.find("sip:") {
        Some(i) => &s[i + 4..],
        None => s.trim_matches(|c: char| c == '<' || c == '>' || c == '"' || c.is_whitespace()),
    };
    let end = after_sip
        .find(|c: char| c == '@' || c == '>' || c == ';' || c == ' ')
        .unwrap_or(after_sip.len());
    after_sip[..end].to_string()
}

/// Map a container-relative recording path (`/logs/recordings/foo.wav`) to a
/// host path under `{base}/{instance}/recordings/foo.wav`.
fn resolve_recording_host_path(
    container_path: &std::path::Path,
    instance: u8,
    base: &std::path::Path,
) -> std::path::PathBuf {
    let file_name = container_path
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "recording.wav".to_string());
    base.join(instance.to_string())
        .join("recordings")
        .join(file_name)
}

async fn handle_modem_event(
    ev: super::ModemEvent,
    sse: &Arc<SseManager>,
    webhook: Option<&webhook::WebhookManager>,
    transcribe_cfg: Option<&TranscribeConfig>,
    instance_by_sim: &Arc<RwLock<HashMap<String, u8>>>,
    recordings_base: &std::path::Path,
) {
    use super::ModemEvent;
    match ev {
        ModemEvent::SmsReceived { sim_id, from, body, timestamp } => {
            let contact = extract_phone(&from);
            // Empty body and no parseable sender is carrier noise (e.g. RP-ACK);
            // skip persistence but still leave a trace for debugging.
            if body.is_empty() && contact.is_empty() {
                log::debug!("[ami {}] SmsReceived dropped (empty from+body)", sim_id);
                return;
            }
            let sms = ModemSMS {
                contact: contact.clone(),
                timestamp: timestamp.naive_utc(),
                message: body,
                send: false,
                sim_id: sim_id.clone(),
            };
            if let Some(wh) = webhook {
                if let Err(e) = wh.send(sms.clone()) {
                    log::error!("[ami {}] webhook send failed: {}", sim_id, e);
                }
            }
            match sms.insert().await {
                Ok(_) => {
                    info!("[ami {}] SMS from {} stored", sim_id, contact);
                    if let Ok(Some(contact_id)) = Contact::find_id_by_name(&contact).await {
                        if let Ok(conversations) =
                            crate::db::Conversation::query_by_contact_ids(&[contact_id]).await
                        {
                            sse.send(conversations);
                        }
                    }
                }
                Err(e) => log::error!("[ami {}] failed to insert SMS: {}", sim_id, e),
            }
        }

        ModemEvent::CallRinging { sim_id, call_id, phone, direction } => {
            let phone_opt = if phone.is_empty() { None } else { Some(phone.as_str()) };
            if let Err(e) =
                Call::insert_with_id(&call_id, &sim_id, phone_opt, &direction).await
            {
                log::error!("[ami {}] failed to insert call {}: {}", sim_id, call_id, e);
                return;
            }
            let event_type = if direction == "outbound" {
                "outbound_call"
            } else {
                "incoming_call"
            }
            .to_string();
            sse.send_call_event(CallEvent {
                event_type,
                sim_id,
                call_id,
                phone: phone_opt.map(|s| s.to_string()),
                direction,
            });
        }

        ModemEvent::CallAnswered { sim_id, call_id } => {
            if let Err(e) = Call::update_status(&call_id, "active").await {
                log::error!("[ami {}] failed to mark call {} active: {}", sim_id, call_id, e);
            }
            sse.send_call_event(CallEvent {
                event_type: "call_answered".into(),
                sim_id,
                call_id,
                phone: None,
                direction: "".into(),
            });
        }

        ModemEvent::CallEnded { sim_id, call_id, recording_path } => {
            if let Err(e) = Call::update_status(&call_id, "ended").await {
                log::error!("[ami {}] failed to mark call {} ended: {}", sim_id, call_id, e);
            }
            sse.send_call_event(CallEvent {
                event_type: "call_ended".into(),
                sim_id: sim_id.clone(),
                call_id: call_id.clone(),
                phone: None,
                direction: "".into(),
            });
            // Recording arrives as a container-relative path; resolve to host
            // and spawn the load → save_recording → transcribe pipeline.
            if let Some(rp) = recording_path {
                let instance = instance_by_sim.read().await.get(&sim_id).copied();
                let Some(instance) = instance else {
                    log::warn!(
                        "[ami {}] CallEnded recording_path={:?} but no instance mapping; skipping",
                        sim_id, rp
                    );
                    return;
                };
                let host_path = resolve_recording_host_path(&rp, instance, recordings_base);
                let cfg_owned = transcribe_cfg.cloned();
                tokio::spawn(async move {
                    if let Err(e) =
                        ingest_recording(sim_id, call_id, host_path, cfg_owned).await
                    {
                        log::error!("[ami] recording ingest failed: {}", e);
                    }
                });
            }
        }

        ModemEvent::RecordingDone { sim_id, call_id, path } => {
            let cfg_owned = transcribe_cfg.cloned();
            tokio::spawn(async move {
                if let Err(e) = ingest_recording(sim_id, call_id, path, cfg_owned).await {
                    log::error!("[ami] recording ingest failed: {}", e);
                }
            });
        }
    }
}

/// Wait for a recording file to settle, read it, save to DB, and (optionally)
/// transcribe with whisper.cpp.
async fn ingest_recording(
    sim_id: String,
    call_id: String,
    host_path: std::path::PathBuf,
    transcribe_cfg: Option<TranscribeConfig>,
) -> anyhow::Result<()> {
    // MixMonitor finalizes the file on Hangup; give it a moment to flush.
    for _ in 0..20 {
        if tokio::fs::try_exists(&host_path).await.unwrap_or(false) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    let data = tokio::fs::read(&host_path)
        .await
        .map_err(|e| anyhow::anyhow!("read {}: {}", host_path.display(), e))?;
    info!(
        "[ami {}] recording {} -> {} bytes",
        sim_id,
        host_path.display(),
        data.len()
    );
    Call::save_recording(&call_id, &data).await?;

    if let Some(cfg) = transcribe_cfg {
        let t = std::time::Instant::now();
        match crate::transcribe::transcribe(
            &data,
            &cfg.ffmpeg_exe,
            &cfg.whisper_exe,
            &cfg.whisper_model,
            &cfg.whisper_language,
        )
        .await
        {
            Ok(text) => {
                info!(
                    "[ami {}] transcript ({:.1}s): {}",
                    sim_id,
                    t.elapsed().as_secs_f64(),
                    text
                );
                Call::save_transcript(&call_id, &text).await?;
            }
            Err(e) => error!("[ami {}] transcribe failed: {}", sim_id, e),
        }
    }
    Ok(())
}
