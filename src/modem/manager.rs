use futures::stream::{FuturesUnordered, StreamExt};
use log::{error, info};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{RwLock, Semaphore};

use crate::api::sse_manager::CallEvent;
use crate::api::SseManager;
use crate::config::{Settings, SmsStorage};
use crate::db::{Call, Contact, ModemSMS, SimCard};
use crate::webhook;

use super::core::Modem;
use super::types::*;

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

pub struct ModemManager {
    modems: Arc<RwLock<HashMap<String, Arc<Modem>>>>,
    /// AMI-mode transports keyed by sim_id (ICCID or fallback instance id).
    transports: Arc<RwLock<HashMap<String, Arc<dyn super::Transport>>>>,
    /// sim_id -> asterisk container instance number (1..=N). Used to map
    /// container-relative RecordingPath (`/logs/recordings/foo.wav`) to a host
    /// path under `{recordings_base_dir}/{instance}/recordings/foo.wav`.
    instance_by_sim: Arc<RwLock<HashMap<String, u8>>>,
    /// Host base dir for the per-instance `/logs/` volumes.
    recordings_base_dir: std::path::PathBuf,
    sim_cards_cache: Arc<RwLock<HashMap<String, SimCard>>>,
    _initialization_semaphore: Arc<Semaphore>,
    /// COM ports that failed to open at startup (com_port, baud_rate)
    pub unavailable_ports: Vec<(String, u32)>,
    /// Receiver end of the channel that AmiTransport instances push ModemEvents into.
    /// Wrapped in Mutex<Option<...>> so `start_event_handlers` can take exclusive ownership.
    event_rx: tokio::sync::Mutex<Option<tokio::sync::mpsc::Receiver<super::ModemEvent>>>,
}

impl ModemManager {
    pub async fn initialize(config: &crate::config::AppConfig) -> anyhow::Result<Self> {
        let initialization_semaphore = Arc::new(Semaphore::new(3));
        let mut initialization_futures = FuturesUnordered::new();

        // ── AMI-mode transports ──────────────────────────────────────────────
        // mpsc channel that all AmiTransports push ModemEvent into; the receiver
        // is consumed by start_event_handlers().
        let (event_tx, event_rx) = tokio::sync::mpsc::channel::<super::ModemEvent>(1024);
        let mut transports: HashMap<String, Arc<dyn super::Transport>> = HashMap::new();
        let mut instance_by_sim: HashMap<String, u8> = HashMap::new();

        for (index, device) in config.devices.iter().enumerate() {
            let has_ami = device.instance.is_some()
                || device.ami_port.is_some()
                || device.ami_host.is_some();
            if !has_ami {
                continue;
            }
            let instance = device.instance.unwrap_or((index + 1) as u8);
            let ami_host = device
                .ami_host
                .clone()
                .unwrap_or_else(|| "127.0.0.1".to_string());
            let ami_port = device
                .ami_port
                .unwrap_or(5037u16 + instance as u16);
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
                            path,
                            index,
                            e
                        );
                        continue;
                    }
                },
                None => device
                    .ami_secret
                    .clone()
                    .unwrap_or_else(|| "geheim".to_string()),
            };

            // Stable sim_id: prefer configured ICCID; otherwise instance-based fallback.
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
                },
                event_tx.clone(),
            );
            transports.insert(sim_id.clone(), Arc::new(transport));
            instance_by_sim.insert(sim_id.clone(), instance);
            info!("Registered AMI transport for device {} (sim_id={})", index, sim_id);
        }
        // Drop the spare event_tx so the receiver knows when all transports go away.
        drop(event_tx);

        // ── Legacy serial path ───────────────────────────────────────────────
        for (index, device) in config.devices.iter().enumerate() {
            // Legacy serial path: requires both com_port and baud_rate to be set.
            // AMI-mode devices skip serial init entirely (handled by upcoming
            // Transport refactor).
            let Some(port) = device.com_port.clone() else {
                log::info!(
                    "Device {} has no com_port; skipping legacy serial init (AMI-mode)",
                    index
                );
                continue;
            };
            let Some(baud_rate) = device.baud_rate else {
                log::warn!(
                    "Device {} ({}) has no baud_rate; skipping",
                    index,
                    port
                );
                continue;
            };
            let sms_storage = device.sms_storage.or(config.settings.sms_storage);
            let temp_device_id = format!("device_{}", index);
            let semaphore = initialization_semaphore.clone();

            initialization_futures.push(async move {
                let _permit = semaphore.acquire().await;
                Self::initialize_single_modem(port.clone(), baud_rate, temp_device_id, sms_storage, index)
                    .await
                    .map_err(|e| (port, baud_rate, e))
            });
        }

        let mut modems = HashMap::new();
        let mut new_sim_ids = Vec::new();
        let mut unavailable_ports: Vec<(String, u32)> = Vec::new();

        while let Some(result) = initialization_futures.next().await {
            match result {
                Ok((sim_id, modem, is_new)) => {
                    if is_new {
                        new_sim_ids.push(sim_id.clone());
                    }
                    modems.insert(sim_id, Arc::new(modem));
                }
                Err((port, baud_rate, e)) => {
                    error!("Failed to initialize modem on {}: {}", port, e);
                    unavailable_ports.push((port, baud_rate));
                }
            }
        }

        if modems.is_empty() && transports.is_empty() {
            return Err(anyhow::anyhow!("No modems or AMI transports were successfully initialized"));
        }

        info!(
            "Initialized: {} serial modem(s), {} AMI transport(s), {} unavailable serial port(s)",
            modems.len(),
            transports.len(),
            unavailable_ports.len()
        );

        let manager = Self {
            modems: Arc::new(RwLock::new(modems)),
            transports: Arc::new(RwLock::new(transports)),
            instance_by_sim: Arc::new(RwLock::new(instance_by_sim)),
            recordings_base_dir: config
                .settings
                .recordings_base_dir
                .clone()
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| std::path::PathBuf::from("/home/ht/docker/logs")),
            sim_cards_cache: Arc::new(RwLock::new(HashMap::new())),
            _initialization_semaphore: initialization_semaphore,
            unavailable_ports,
            event_rx: tokio::sync::Mutex::new(Some(event_rx)),
        };

        manager.init_sim_cache().await?;

        if !new_sim_ids.is_empty() {
            manager.init_new_sim_sms_data(new_sim_ids).await;
        }

        Ok(manager)
    }

    async fn initialize_single_modem(
        port: String,
        baud_rate: u32,
        device_id: String,
        sms_storage: Option<SmsStorage>,
        index: usize,
    ) -> anyhow::Result<(String, Modem, bool)> {
        info!("Initializing modem on port {}", port);

        let modem = Modem::new(&port, baud_rate, &device_id, index).await?;

        let pre_sim_id = modem.get_sim_iccid().await.ok().flatten();
        let is_new_sim = if let Some(ref sim_id) = pre_sim_id {
            Self::is_new_sim_id(sim_id).await
        } else {
            false
        };

        if let Err(e) = modem.init_modem(sms_storage).await {
            log::warn!(
                "Modem AT init failed for port {} (no SIM inserted?): {}. \
                 Adding as partial entry.",
                port,
                e
            );
        }

        let sim_id = pre_sim_id.unwrap_or_else(|| {
            log::warn!("Using fallback SIM ID for port {}", port);
            format!("fallback_sim_{}", index)
        });

        info!(
            "Successfully initialized modem on {} with SIM ID: {}",
            port, sim_id
        );

        Ok((sim_id, modem, is_new_sim))
    }

    async fn is_new_sim_id(sim_id: &str) -> bool {
        match SimCard::find_by_conditions(Some(sim_id), None, None, None).await {
            Ok(existing) => existing.is_empty(),
            Err(e) => {
                log::warn!("Failed to check SIM ID existence: {}", e);
                true
            }
        }
    }

    async fn init_sim_cache(&self) -> anyhow::Result<()> {
        let modems = self.modems.read().await;
        let sim_ids: Vec<&str> = modems.keys().map(|k| k.as_str()).collect();

        let sim_cards = SimCard::get_by_ids(&sim_ids).await?;

        let mut cache = self.sim_cards_cache.write().await;
        *cache = sim_cards;

        info!("Initialized SIM cache with {} cards", cache.len());
        Ok(())
    }

    async fn init_new_sim_sms_data(&self, new_sim_ids: Vec<String>) {
        let mut futures = FuturesUnordered::new();

        for sim_id in new_sim_ids {
            let modems = self.modems.clone();
            futures.push(async move {
                let modems = modems.read().await;
                if let Some(modem) = modems.get(&sim_id) {
                    match modem.read_sms_sync_insert(SmsType::All).await {
                        Ok(()) => info!("Initialized SMS data for new SIM: {}", sim_id),
                        Err(e) => error!("Failed to initialize SMS data for {}: {}", sim_id, e),
                    }
                }
            });
        }

        while futures.next().await.is_some() {}
    }

    pub async fn get_sim_ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = self.modems.read().await.keys().cloned().collect();
        for k in self.transports.read().await.keys() {
            if !ids.contains(k) {
                ids.push(k.clone());
            }
        }
        ids
    }

    pub async fn get_modem(&self, sim_id: &str) -> Option<Arc<Modem>> {
        self.modems.read().await.get(sim_id).cloned()
    }

    /// Returns the AMI/abstract transport for this sim, if any.
    pub async fn get_transport(&self, sim_id: &str) -> Option<Arc<dyn super::Transport>> {
        self.transports.read().await.get(sim_id).cloned()
    }

    /// Take ownership of the event receiver. Returns None if already taken.
    /// The consumer should be the event handler spawned by `start_event_handlers`.
    pub async fn take_event_rx(&self) -> Option<tokio::sync::mpsc::Receiver<super::ModemEvent>> {
        self.event_rx.lock().await.take()
    }

    pub async fn send_sms(
        &self,
        sim_id: &str,
        contact: &Contact,
        message: &str,
    ) -> anyhow::Result<(i64, String)> {
        let modem = self
            .get_modem(sim_id)
            .await
            .ok_or_else(|| anyhow::anyhow!("Modem not found for SIM ID: {}", sim_id))?;

        modem.send_sms_pdu(contact, message).await
    }

    pub async fn read_sms(&self, sim_id: &str, sms_type: SmsType) -> anyhow::Result<Vec<ModemSMS>> {
        let modem = self
            .get_modem(sim_id)
            .await
            .ok_or_else(|| anyhow::anyhow!("Modem not found for SIM ID: {}", sim_id))?;

        modem.read_sms(sms_type).await.map_err(Into::into)
    }

    pub async fn read_sms_async_insert(
        &self,
        sim_id: &str,
        sms_type: SmsType,
        sse_manager: Arc<SseManager>,
        webhook_manager: Option<webhook::WebhookManager>,
    ) -> anyhow::Result<()> {
        let modem = self
            .get_modem(sim_id)
            .await
            .ok_or_else(|| anyhow::anyhow!("Modem not found for SIM ID: {}", sim_id))?;

        modem
            .read_sms_async_insert(sms_type, sse_manager, webhook_manager)
            .await
    }

    pub async fn read_sms_sync_insert(
        &self,
        sim_id: &str,
        sms_type: SmsType,
    ) -> anyhow::Result<()> {
        let modem = self
            .get_modem(sim_id)
            .await
            .ok_or_else(|| anyhow::anyhow!("Modem not found for SIM ID: {}", sim_id))?;

        modem.read_sms_sync_insert(sms_type).await
    }

    pub async fn read_all_sms_async(
        &self,
        sms_type: SmsType,
        sse_manager: Arc<SseManager>,
        webhook_manager: Option<webhook::WebhookManager>,
    ) {
        let modems = self.modems.read().await;
        let mut futures = FuturesUnordered::new();

        for (sim_id, modem) in modems.iter() {
            // Skip modems without a SIM card — they cannot receive SMS
            if sim_id.starts_with("fallback_sim_") {
                continue;
            }

            let modem = modem.clone();
            let sse_manager = sse_manager.clone();
            let webhook_manager = webhook_manager.clone();
            let sim_id = sim_id.clone();

            futures.push(async move {
                if let Err(e) = modem
                    .read_sms_async_insert(sms_type, sse_manager, webhook_manager)
                    .await
                {
                    error!("Failed to read SMS for {}: {}", sim_id, e);
                }
            });
        }

        while futures.next().await.is_some() {}
    }

    /// Re-check all modems in parallel for SIM insertion/removal.
    /// Demotes real-ICCID modems where +CCID returns a different or missing ICCID.
    /// Promotes fallback modems where +CCID now returns an ICCID.
    pub async fn recheck_fallback_modems(
        &self,
        sms_storage_map: &std::collections::HashMap<String, Option<crate::config::SmsStorage>>,
        sse_manager: Arc<SseManager>,
        webhook_manager: Option<webhook::WebhookManager>,
        transcribe_cfg: Option<Arc<TranscribeConfig>>,
    ) {
        // ── Demotion: parallel AT+CCID on all real-ICCID modems ──────────────
        let active_entries: Vec<(String, Arc<Modem>)> = {
            let modems = self.modems.read().await;
            modems
                .iter()
                .filter(|(k, _)| !k.starts_with("fallback_sim_"))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect()
        };

        let mut demotion_futs = FuturesUnordered::new();
        for (iccid, modem) in active_entries {
            demotion_futs.push(async move {
                match modem.get_sim_iccid().await {
                    Ok(Some(current_iccid)) if current_iccid == iccid => None,
                    Ok(Some(current_iccid)) => {
                        info!(
                            "SIM swap detected on {} (was {}, now {}). Forcing re-init.",
                            modem.com_port, iccid, current_iccid
                        );
                        Some((iccid, modem.fallback_key.clone(), modem.com_port.clone()))
                    }
                    _ => Some((iccid, modem.fallback_key.clone(), modem.com_port.clone())),
                }
            });
        }
        let mut demotions = Vec::new();
        while let Some(result) = demotion_futs.next().await {
            if let Some(d) = result {
                demotions.push(d);
            }
        }
        for (iccid, fallback_key, com_port) in demotions {
            info!(
                "SIM removed from {} (was {}). Demoting to {}.",
                com_port, iccid, fallback_key
            );
            let mut modems = self.modems.write().await;
            if let Some(m) = modems.remove(&iccid) {
                modems.insert(fallback_key, m);
            }
        }

        // ── Promotion: parallel AT+CCID on all fallback modems ────────────────
        let fallback_entries: Vec<(String, Arc<Modem>)> = {
            let modems = self.modems.read().await;
            modems
                .iter()
                .filter(|(k, _)| k.starts_with("fallback_sim_"))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect()
        };

        let mut promotion_futs = FuturesUnordered::new();
        for (fallback_key, modem) in fallback_entries {
            let sms_storage = sms_storage_map.get(&modem.com_port).copied().flatten();
            promotion_futs.push(async move {
                let iccid = match modem.get_sim_iccid().await {
                    Ok(Some(id)) => id,
                    _ => return None,
                };
                info!(
                    "SIM detected on {} (was {}): ICCID={}. Re-initializing.",
                    modem.com_port, fallback_key, iccid
                );
                if let Err(e) = modem.init_modem(sms_storage).await {
                    log::warn!(
                        "Re-init modem on {} failed: {}. Will retry next cycle.",
                        modem.com_port,
                        e
                    );
                    return None;
                }
                Some((fallback_key, iccid, modem.com_port.clone()))
            });
        }
        while let Some(result) = promotion_futs.next().await {
            if let Some((fallback_key, iccid, com_port)) = result {
                let promoted_modem = {
                    let mut modems = self.modems.write().await;
                    if let Some(m) = modems.remove(&fallback_key) {
                        modems.insert(iccid.clone(), m.clone());
                        info!("Modem on {} promoted: {} -> {}", com_port, fallback_key, iccid);
                        Some(m)
                    } else {
                        None
                    }
                };
                // Immediately read any SMS that arrived on the new SIM
                if let Some(modem) = promoted_modem {
                    let sse = sse_manager.clone();
                    let wh = webhook_manager.clone();
                    let modem_sms = modem.clone();
                    let com_port_sms = com_port.clone();
                    tokio::spawn(async move {
                        if let Err(e) = modem_sms.read_sms_async_insert(SmsType::All, sse, wh).await {
                            log::warn!("Failed to read SMS after SIM swap on {}: {}", com_port_sms, e);
                        }
                    });
                    // Spawn a URC handler for the newly promoted modem so it can
                    // receive RING / NO CARRIER events on this line going forward.
                    tokio::spawn(Self::run_urc_handler(
                        iccid.clone(),
                        modem,
                        sse_manager.clone(),
                        transcribe_cfg.clone(),
                    ));
                }
            }
        }
    }

    pub async fn get_signal_quality(&self, sim_id: &str) -> anyhow::Result<Option<SignalQuality>> {
        let modem = self
            .get_modem(sim_id)
            .await
            .ok_or_else(|| anyhow::anyhow!("Modem not found for SIM ID: {}", sim_id))?;

        modem.get_signal_quality().await.map_err(Into::into)
    }

    pub async fn check_network_registration(
        &self,
        sim_id: &str,
    ) -> anyhow::Result<Option<NetworkRegistrationStatus>> {
        let modem = self
            .get_modem(sim_id)
            .await
            .ok_or_else(|| anyhow::anyhow!("Modem not found for SIM ID: {}", sim_id))?;

        modem.check_network_registration().await.map_err(Into::into)
    }

    pub async fn check_operator(&self, sim_id: &str) -> anyhow::Result<Option<OperatorInfo>> {
        let modem = self
            .get_modem(sim_id)
            .await
            .ok_or_else(|| anyhow::anyhow!("Modem not found for SIM ID: {}", sim_id))?;

        modem.check_operator().await.map_err(Into::into)
    }

    pub async fn get_modem_model(&self, sim_id: &str) -> anyhow::Result<Option<ModemInfo>> {
        let modem = self
            .get_modem(sim_id)
            .await
            .ok_or_else(|| anyhow::anyhow!("Modem not found for SIM ID: {}", sim_id))?;

        modem.get_modem_model().await.map_err(Into::into)
    }

    pub async fn get_imei(&self, sim_id: &str) -> anyhow::Result<Option<String>> {
        let modem = self
            .get_modem(sim_id)
            .await
            .ok_or_else(|| anyhow::anyhow!("Modem not found for SIM ID: {}", sim_id))?;

        modem.get_imei().await.map_err(Into::into)
    }

    pub async fn get_sms_center(&self, sim_id: &str) -> anyhow::Result<Option<String>> {
        let modem = self
            .get_modem(sim_id)
            .await
            .ok_or_else(|| anyhow::anyhow!("Modem not found for SIM ID: {}", sim_id))?;

        modem.get_sms_center().await.map_err(Into::into)
    }

    pub async fn get_network_info(&self, sim_id: &str) -> anyhow::Result<Option<String>> {
        let modem = self
            .get_modem(sim_id)
            .await
            .ok_or_else(|| anyhow::anyhow!("Modem not found for SIM ID: {}", sim_id))?;

        modem.get_network_info().await.map_err(Into::into)
    }

    pub async fn get_sim_status(&self, sim_id: &str) -> anyhow::Result<Option<String>> {
        let modem = self
            .get_modem(sim_id)
            .await
            .ok_or_else(|| anyhow::anyhow!("Modem not found for SIM ID: {}", sim_id))?;

        modem.get_sim_status().await.map_err(Into::into)
    }

    pub async fn get_memory_status(&self, sim_id: &str) -> anyhow::Result<Option<String>> {
        let modem = self
            .get_modem(sim_id)
            .await
            .ok_or_else(|| anyhow::anyhow!("Modem not found for SIM ID: {}", sim_id))?;

        modem.get_memory_status().await.map_err(Into::into)
    }

    pub async fn get_temperature_info(&self, sim_id: &str) -> anyhow::Result<Option<String>> {
        let modem = self
            .get_modem(sim_id)
            .await
            .ok_or_else(|| anyhow::anyhow!("Modem not found for SIM ID: {}", sim_id))?;

        modem.get_temperature_info().await.map_err(Into::into)
    }

    pub async fn set_sms_storage(
        &self,
        sim_id: &str,
        sms_storage: SmsStorage,
    ) -> anyhow::Result<()> {
        let modem = self
            .get_modem(sim_id)
            .await
            .ok_or_else(|| anyhow::anyhow!("Modem not found for SIM ID: {}", sim_id))?;

        modem.set_sms_storage(sms_storage).await.map_err(Into::into)
    }

    pub async fn get_sms_storage_status(&self, sim_id: &str) -> anyhow::Result<Option<String>> {
        let modem = self
            .get_modem(sim_id)
            .await
            .ok_or_else(|| anyhow::anyhow!("Modem not found for SIM ID: {}", sim_id))?;

        modem.get_sms_storage_status().await.map_err(Into::into)
    }

    pub async fn get_sim_card_cached(&self, sim_id: &str) -> Option<SimCard> {
        self.sim_cards_cache.read().await.get(sim_id).cloned()
    }

    /// Find the sim_id (ICCID) that matches a given SIM phone number.
    /// Searches the in-memory cache, so the SIM must have been seen at least once.
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

    // ─── Voice call delegation ────────────────────────────────────────────────

    /// Initiate an outbound call and record it in the DB.
    /// Returns the new Call UUID on success.
    pub async fn make_call(&self, sim_id: &str, phone: &str, sse: Arc<SseManager>) -> anyhow::Result<String> {
        let modem = self
            .get_modem(sim_id)
            .await
            .ok_or_else(|| anyhow::anyhow!("Modem not found for SIM ID: {}", sim_id))?;

        // Cancel any existing outbound poller and finalize the previous call as missed.
        // This prevents the old poller's poll_cancel_rx from firing when we replace
        // outbound_poll_cancel_tx, which would leave the previous call stuck as 'ringing'.
        if let Some(tx) = modem.outbound_poll_cancel_tx.lock().await.take() {
            let _ = tx.send(());
        }
        if let Some(old_id) = modem.outbound_call_id.lock().await.take() {
            if let Err(e) = Call::update_status(&old_id, "missed").await {
                error!("[{}] failed to finalize previous outbound call {}: {}", sim_id, old_id, e);
            }
            sse.send_call_event(CallEvent {
                event_type: "call_ended".into(),
                sim_id: sim_id.to_string(),
                call_id: old_id,
                phone: None,
                direction: "outbound".into(),
            });
        }

        // Send the ATD command first — only record if the modem accepts it.
        modem.make_call(phone).await?;
        // Insert a DB record now that the modem confirmed OK.
        let call_id = Call::insert(sim_id, Some(phone), "outbound").await?;
        // Reset per-call state.
        *modem.outbound_call_answered.lock().await = false;
        *modem.outbound_call_id.lock().await = Some(call_id.clone());
        info!("[{}] outbound call {} to {} started", sim_id, call_id, phone);

        // Spawn AT+CLCC polling task — sole mechanism for detecting outbound call state.
        // EC20F does not reliably emit NO CARRIER for rejected calls; NO CARRIER is
        // also unreliable when inbound calls are active (it gets consumed by the inbound handler).
        let call_id_c = call_id.clone();
        let phone_c   = phone.to_string();
        let modem_c   = modem.clone();
        let sim_id_c  = sim_id.to_string();
        let sse_c     = sse;
        let (poll_cancel_tx, mut poll_cancel_rx) = tokio::sync::oneshot::channel::<()>();
        *modem.outbound_poll_cancel_tx.lock().await = Some(poll_cancel_tx);

        tokio::spawn(async move {
            let mut call_answered     = false;
            let mut consecutive_empty: u8 = 0;
            info!("[CLCC {}] polling started for call {}", sim_id_c, call_id_c);
            loop {
                tokio::select! {
                    result = &mut poll_cancel_rx => {
                        // Cancelled explicitly by hangup_call() — finalize properly.
                        // Ignore if sender was dropped (overlapping call scenario already handled).
                        if result.is_ok() {
                            info!("[CLCC {}] polling cancelled (user hung up)", sim_id_c);
                        }
                        break;
                    }
                    _ = tokio::time::sleep(Duration::from_secs(1)) => {}
                }
                match modem_c.query_clcc().await {
                    Ok(ref resp) => {
                        if let Some(stat) = Self::parse_clcc_mo_stat(resp) {
                            // MO call still in CLCC — reset empty counter.
                            consecutive_empty = 0;
                            match stat {
                                0 if !call_answered => {
                                    call_answered = true;
                                    *modem_c.outbound_call_answered.lock().await = true;
                                    if let Err(e) = Call::update_status(&call_id_c, "active").await {
                                        error!("[CLCC {}] failed to set active: {}", sim_id_c, e);
                                    }
                                    sse_c.send_call_event(CallEvent {
                                        event_type: "call_answered".into(),
                                        sim_id:     sim_id_c.clone(),
                                        call_id:    call_id_c.clone(),
                                        phone:      Some(phone_c.clone()),
                                        direction:  "outbound".into(),
                                    });
                                    info!("[CLCC {}] call {} answered by remote", sim_id_c, call_id_c);
                                }
                                3 => info!("[CLCC {}] remote ringing", sim_id_c),
                                2 => info!("[CLCC {}] dialing", sim_id_c),
                                _ => {}
                            }
                        } else {
                            // No MO (dir=0) entry — outbound call is gone.
                            consecutive_empty += 1;
                            info!("[CLCC {}] no MO call in CLCC ({}/2)", sim_id_c, consecutive_empty);
                            if consecutive_empty >= 2 {
                                let status = if call_answered { "ended" } else { "missed" };
                                // Always update using call_id_c (captured) — never the mutex,
                                // which may have been replaced by a newer call.
                                if let Err(e) = Call::update_status(&call_id_c, status).await {
                                    error!("[CLCC {}] failed to finalize call {}: {}", sim_id_c, call_id_c, e);
                                }
                                // Remove from mutex only if it still holds our call_id.
                                let mut lock = modem_c.outbound_call_id.lock().await;
                                if lock.as_deref() == Some(call_id_c.as_str()) {
                                    *lock = None;
                                }
                                drop(lock);
                                sse_c.send_call_event(CallEvent {
                                    event_type: "call_ended".into(),
                                    sim_id:     sim_id_c.clone(),
                                    call_id:    call_id_c.clone(),
                                    phone:      Some(phone_c.clone()),
                                    direction:  "outbound".into(),
                                });
                                info!("[CLCC {}] call_ended SSE sent (status={})", sim_id_c, status);
                                break;
                            }
                        }
                    }
                    Err(e) => {
                        error!("[CLCC {}] AT+CLCC failed: {}", sim_id_c, e);
                    }
                }
            }
            info!("[CLCC {}] polling ended", sim_id_c);
        });

        Ok(call_id)
    }

    /// Parse the stat field from the first +CLCC line with dir=0 (MO = mobile-originated).
    /// stat: 0=active, 1=held, 2=dialing(MO), 3=alerting/ringing(MO), 4=incoming(MT), 5=waiting.
    /// Returns None if no MO call is present (CLCC empty or only MT entries).
    fn parse_clcc_mo_stat(response: &str) -> Option<u8> {
        for line in response.lines() {
            let line = line.trim();
            if let Some(rest) = line.strip_prefix("+CLCC:") {
                // Format: <idx>,<dir>,<stat>,<mode>,<mpty>[,"<num>",<type>]
                let parts: Vec<&str> = rest.split(',').collect();
                if parts.len() >= 3 {
                    let dir: u8 = parts[1].trim().parse().unwrap_or(255);
                    if dir == 0 {  // 0 = MO (mobile originated)
                        if let Ok(stat) = parts[2].trim().parse::<u8>() {
                            return Some(stat);
                        }
                    }
                }
            }
        }
        None
    }

    pub async fn answer_call(&self, sim_id: &str) -> anyhow::Result<()> {
        let modem = self
            .get_modem(sim_id)
            .await
            .ok_or_else(|| anyhow::anyhow!("Modem not found for SIM ID: {}", sim_id))?;
        modem.answer_call().await.map_err(Into::into)
    }

    /// Hang up the current call.  Returns the call_id and final status of the
    /// outbound call if one was in progress, so the caller can send an SSE event.
    pub async fn hangup_call(&self, sim_id: &str) -> anyhow::Result<Option<(String, String)>> {
        let modem = self
            .get_modem(sim_id)
            .await
            .ok_or_else(|| anyhow::anyhow!("Modem not found for SIM ID: {}", sim_id))?;
        modem.hangup_call().await?;
        // Cancel the CLCC polling task so it does not race with our finalization below.
        if let Some(tx) = modem.outbound_poll_cancel_tx.lock().await.take() {
            let _ = tx.send(());
        }
        // Take any pending outbound call and finalize it.
        if let Some(call_id) = modem.outbound_call_id.lock().await.take() {
            let answered = *modem.outbound_call_answered.lock().await;
            let status = if answered { "ended" } else { "missed" };
            if let Err(e) = Call::update_status(&call_id, status).await {
                error!("[{}] failed to update outbound call {} on hangup: {}", sim_id, call_id, e);
            } else {
                info!("[{}] outbound call {} marked {} on hangup", sim_id, call_id, status);
            }
            return Ok(Some((call_id, status.to_string())));
        }
        Ok(None)
    }

    // ─── URC handler tasks ────────────────────────────────────────────────────

    /// Spawn one URC handler task per modem.  Must be called once after
    /// initialization.  Each task owns the modem's `urc_rx` and processes
    /// RING / +CLIP: / NO CARRIER / VOICE CALL: END messages forever.
    pub async fn start_urc_handlers(
        &self,
        sse_manager: Arc<SseManager>,
        webhook_manager: Option<webhook::WebhookManager>,
        transcribe_cfg: Option<Arc<TranscribeConfig>>,
    ) {
        // Legacy serial path: spawn one URC reader per AT modem.
        let modems = self.modems.read().await;
        for (sim_id, modem) in modems.iter() {
            if sim_id.starts_with("fallback_sim_") {
                continue;
            }
            let sim_id = sim_id.clone();
            let modem = modem.clone();
            let sse = sse_manager.clone();
            let cfg = transcribe_cfg.clone();
            tokio::spawn(async move {
                Self::run_urc_handler(sim_id, modem, sse, cfg).await;
            });
        }
        drop(modems);

        // AMI path: drain the shared ModemEvent channel and fan each event out
        // to the DB, SSE, webhooks, and (for recordings) the transcription
        // pipeline. Mirrors the responsibilities of the serial URC handler.
        if let Some(mut rx) = self.take_event_rx().await {
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

    async fn run_urc_handler(sim_id: String, modem: Arc<Modem>, sse: Arc<SseManager>, transcribe_cfg: Option<Arc<TranscribeConfig>>) {
        // Hold the receiver for the task's lifetime — one consumer per modem.
        let mut rx = modem.urc_rx.lock().await;
        // active_call_id tracks the current ringing/active call for this modem.
        let mut active_call_id: Option<String> = None;
        // Phone number arrives via +CLIP: *after* the first RING on EC20 modems.
        let mut current_phone: Option<String> = None;
        // Whether ATA was sent (answered); used to distinguish missed vs ended.
        let mut call_answered = false;
        // Oneshot sender to cancel the 30-second recording timer. Some = recording active.
        let mut recording_cancel_tx: Option<tokio::sync::oneshot::Sender<()>> = None;

        info!("[URC {}] handler started", sim_id);

        loop {
            let line = match rx.recv().await {
                Some(l) => l,
                None => {
                    info!("[URC {}] channel closed, handler exiting", sim_id);
                    break;
                }
            };

            let line = line.trim().to_string();
            if line.is_empty() {
                continue;
            }

            if line.starts_with("+CLIP:") {
                if let Some(phone) = Modem::parse_clip(&line) {
                    // If a call record already exists (RING arrived before +CLIP:), update its phone.
                    if let Some(ref id) = active_call_id {
                        if let Err(e) = Call::update_phone(id, &phone).await {
                            error!("[URC {}] failed to update phone on call {}: {}", sim_id, id, e);
                        }
                        // Also push an updated SSE event so the frontend shows the number.
                        sse.send_call_event(CallEvent {
                            event_type: "incoming_call".into(),
                            sim_id: sim_id.clone(),
                            call_id: id.clone(),
                            phone: Some(phone.clone()),
                            direction: "inbound".into(),
                        });
                    }
                    current_phone = Some(phone);
                }
                continue;
            }

            if line == "RING" {
                if active_call_id.is_none() {
                    // First RING — insert new inbound call record.
                    // phone may be None here (EC20 sends RING before +CLIP:); we'll patch it when +CLIP: arrives.
                    match Call::insert(&sim_id, current_phone.as_deref(), "inbound").await {
                        Ok(id) => {
                            info!("[URC {}] incoming call {} from {:?}", sim_id, id, current_phone);
                            sse.send_call_event(CallEvent {
                                event_type: "incoming_call".into(),
                                sim_id: sim_id.clone(),
                                call_id: id.clone(),
                                phone: current_phone.clone(),
                                direction: "inbound".into(),
                            });
                            active_call_id = Some(id.clone());
                            call_answered = false;
                            // Check if auto-answer is enabled
                            let should_auto_answer = transcribe_cfg
                                .as_ref()
                                .map(|cfg| cfg.auto_answer_enabled)
                                .unwrap_or(true);
                            if should_auto_answer {
                                // Wait before auto-answering to let the phone ring briefly.
                                if let Some(cfg) = transcribe_cfg.as_ref() {
                                    if cfg.auto_answer_delay_secs > 0 {
                                        info!(
                                            "[URC {}] waiting {}s before auto-answer",
                                            sim_id, cfg.auto_answer_delay_secs
                                        );
                                        tokio::time::sleep(Duration::from_secs(cfg.auto_answer_delay_secs))
                                            .await;
                                    }
                                }
                                // Auto-answer the incoming call.
                                // EC20 voice calls respond to ATA with OK only — no CONNECT URC.
                                // So we handle the answered state here rather than in the CONNECT branch.
                                match modem.answer_call().await {
                                Err(e) => error!("[URC {}] auto-answer failed: {}", sim_id, e),
                                Ok(()) => {
                                    info!("[URC {}] auto-answered incoming call", sim_id);
                                    call_answered = true;
                                    if let Err(e) = Call::update_status(&id, "active").await {
                                        error!("[URC {}] failed to set call {} active: {}", sim_id, id, e);
                                    }
                                    sse.send_call_event(CallEvent {
                                        event_type: "call_answered".into(),
                                        sim_id: sim_id.clone(),
                                        call_id: id.clone(),
                                        phone: current_phone.clone(),
                                        direction: "inbound".into(),
                                    });
                                    // ── Start downlink recording ──────────────────────────
                                    if let Err(e) = modem.delete_files().await {
                                        error!("[URC {}] failed to clear modem UFS: {}", sim_id, e);
                                    }
                                    match modem.start_recording("a.amr").await {
                                        Ok(()) => info!("[URC {}] recording started -> a.amr", sim_id),
                                        Err(e) => error!("[URC {}] failed to start recording: {}", sim_id, e),
                                    }
                                    info!("[URC {}] call_answered={} after start_recording", sim_id, call_answered);
                                    let (cancel_tx, cancel_rx) = tokio::sync::oneshot::channel::<()>();
                                    recording_cancel_tx = Some(cancel_tx);
                                    let modem_c = modem.clone();
                                    let sim_id_c = sim_id.clone();
                                    tokio::spawn(async move {
                                        tokio::select! {
                                            _ = tokio::time::sleep(Duration::from_secs(30)) => {
                                                info!("[URC {}] 30s recording limit, stopping and hanging up", sim_id_c);
                                                modem_c.stop_recording().await.ok();
                                                modem_c.hangup_call().await.ok();
                                                // EC20F may not emit NO CARRIER after ATH — inject
                                                // a synthetic one so the URC handler always cleans up.
                                                modem_c.inject_urc("NO CARRIER");
                                            }
                                            _ = cancel_rx => {
                                                info!("[URC {}] recording timer cancelled", sim_id_c);
                                            }
                                        }
                                    });
                                    // ─────────────────────────────────────────────────────
                                }
                            }
                        } else {
                            info!("[URC {}] auto-answer disabled, call waiting for manual answer", sim_id);
                        }
                        }
                        Err(e) => error!("[URC {}] failed to insert call: {}", sim_id, e),
                    }
                }
                // Subsequent RINGs for the same call are ignored (DB record already exists).
                continue;
            }

            // CONNECT URC — outbound call answered by remote.
            // (Inbound auto-answer is handled directly in the RING branch above.)
            if line == "ATA" || line == "CONNECT" {
                if let Some(outbound_id) = modem.outbound_call_id.lock().await.clone() {
                    call_answered = true;
                    if let Err(e) = Call::update_status(&outbound_id, "active").await {
                        error!("[URC {}] failed to set outbound call {} active: {}", sim_id, outbound_id, e);
                    }
                    sse.send_call_event(CallEvent {
                        event_type: "call_answered".into(),
                        sim_id: sim_id.clone(),
                        call_id: outbound_id,
                        phone: current_phone.clone(),
                        direction: "outbound".into(),
                    });
                }
                continue;
            }

            if line == "NO CARRIER" || line == "BUSY" || line.starts_with("VOICE CALL: END") || line.starts_with("+CEND:") {
                info!("[URC {}] {} received: call_answered={}, has_active={}, has_recording={}",
                    sim_id, line, call_answered, active_call_id.is_some(), recording_cancel_tx.is_some());
                if let Some(call_id) = active_call_id.take() {
                    // ── Stop recording if active ──────────────────────────────
                    if let Some(tx) = recording_cancel_tx.take() {
                        modem.stop_recording().await.ok();
                        tx.send(()).ok(); // cancel the 30s timer task
                        info!("[URC {}] recording stopped", sim_id);
                        // Download the recording from modem UFS and save to DB
                        match modem.download_file("a.amr").await {
                            Ok(data) => {
                                info!("[URC {}] recording downloaded: {} bytes", sim_id, data.len());
                                if let Err(e) = Call::save_recording(&call_id, &data).await {
                                    error!("[URC {}] failed to save recording to DB: {}", sim_id, e);
                                } else {
                                    info!("[URC {}] recording saved to DB", sim_id);
                                    // ── Spawn transcription (fire-and-forget) ──────────
                                    if let Some(cfg) = transcribe_cfg.as_ref() {
                                        let data_c = data.clone();
                                        let call_id_c = call_id.clone();
                                        let sim_id_c = sim_id.clone();
                                        let cfg_c = cfg.clone();
                                        tokio::spawn(async move {
                                            info!("[URC {}] transcribing recording for call {}", sim_id_c, call_id_c);
                                            let t = std::time::Instant::now();
                                            match crate::transcribe::transcribe(
                                                &data_c,
                                                &cfg_c.ffmpeg_exe,
                                                &cfg_c.whisper_exe,
                                                &cfg_c.whisper_model,
                                                &cfg_c.whisper_language,
                                            ).await {
                                                Ok(text) => {
                                                    info!("[URC {}] transcript ({:.1}s): {}", sim_id_c, t.elapsed().as_secs_f64(), text);
                                                    if let Err(e) = Call::save_transcript(&call_id_c, &text).await {
                                                        error!("[URC {}] failed to save transcript: {}", sim_id_c, e);
                                                    }
                                                }
                                                Err(e) => error!("[URC {}] transcription failed ({:.1}s): {}", sim_id_c, t.elapsed().as_secs_f64(), e),
                                            }
                                        });
                                    }
                                    // ───────────────────────────────────────────────────
                                }
                            }
                            Err(e) => error!("[URC {}] failed to download recording: {}", sim_id, e),
                        }
                    }
                    // ─────────────────────────────────────────────────────────
                    // Missed = inbound call that was never answered; Busy = remote busy
                    let new_status = if line == "BUSY" {
                        "missed"
                    } else if !call_answered {
                        "missed"
                    } else {
                        "ended"
                    };
                    call_answered = false;
                    if let Err(e) = Call::update_status(&call_id, new_status).await {
                        error!("[URC {}] failed to update call {}: {}", sim_id, call_id, e);
                    }
                    sse.send_call_event(CallEvent {
                        event_type: "call_ended".into(),
                        sim_id: sim_id.clone(),
                        call_id,
                        phone: current_phone.take(),
                        direction: "inbound".into(),
                    });
                } else {
                    // NO CARRIER with no tracked inbound call.
                    // Outbound call lifecycle is handled entirely by the CLCC poller —
                    // do not interfere here. Just reset local state.
                    call_answered = false;
                    current_phone.take();
                }
                continue;
            }

            // Any other URC (e.g. +CMTI:, +CSQ:) — log at trace level
            log::trace!("[URC {}] unhandled: {:?}", sim_id, line);
        }
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
                    if let Ok(Some(contact_id)) = crate::db::Contact::find_id_by_name(&contact).await {
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
            // and spawn the same load → save_recording → transcribe pipeline as
            // the serial URC handler.
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
            // Serial-style backend that pushes the host path directly.
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
            Err(e) => log::error!("[ami {}] transcribe failed: {}", sim_id, e),
        }
    }
    Ok(())
}
