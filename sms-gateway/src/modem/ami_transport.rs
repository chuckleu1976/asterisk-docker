//! `Transport` implementation that drives one asterisk container via AMI.
//!
//! Maps high-level operations to AMI actions and AMI events to `ModemEvent`s
//! that the rest of the app already consumes.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use log::{debug, info, warn};
use tokio::sync::{mpsc, Mutex, RwLock};
use uuid::Uuid;

use super::ami::{AmiClient, AmiConfig, AmiPacket};
use super::transport::{ModemEvent, SimInfo, Transport};
use super::types::{NetworkRegistrationStatus, OperatorInfo, SmsType};
use crate::db::ModemSMS;
use crate::sim_inventory;

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
    /// 0-based PC/SC reader index (instance - 1). Used to look up IMEI,
    /// MSISDN, MCC/MNC from sim_inventory.db at runtime.
    pub reader_index: u8,
}

pub struct AmiTransport {
    cfg: AmiTransportConfig,
    client: std::sync::Arc<AmiClient>,
    /// Optional handle to the supervisor shutdown channel.
    _shutdown_tx: mpsc::Sender<()>,
    /// Outbound calls we've originated and are awaiting a Channel for.
    /// Key = ChannelId we asked AMI to use; value = our internal call_id.
    pending_originates: Arc<Mutex<HashMap<String, String>>>,
    /// Cached registration state, updated by RegistrationStatus AMI events.
    registration_state: Arc<RwLock<Option<NetworkRegistrationStatus>>>,
    /// Cached SIM identity, refreshed periodically from sim_inventory.db.
    sim_info_cache: Arc<RwLock<super::SimInfo>>,
    /// Phone number last originated via this transport (for hangup lookup).
    pending_phone: Arc<Mutex<Option<String>>>,
}

impl AmiTransport {
    /// Spawn the AMI client supervisor and an event-pump task that forwards
    /// translated events to `events_tx`.
    pub fn spawn(cfg: AmiTransportConfig, events_tx: mpsc::Sender<ModemEvent>) -> Self {
        let (sd_tx, sd_rx) = mpsc::channel::<()>(1);
        let client = std::sync::Arc::new(AmiClient::spawn(cfg.ami.clone(), sd_rx));
        let pending_originates = Arc::new(Mutex::new(HashMap::new()));
        let registration_state: Arc<RwLock<Option<NetworkRegistrationStatus>>> =
            Arc::new(RwLock::new(None));
        let sim_info_cache = Arc::new(RwLock::new(cfg.sim_info.clone()));
        let pending_phone = Arc::new(Mutex::new(None));

        // Event pump: AMI events -> ModemEvent
        let mut rx = client.subscribe();
        let sim_id = cfg.sim_id.clone();
        let label = cfg.ami.label.clone();
        let pending = pending_originates.clone();
        let reg_state = registration_state.clone();
        let s_cache = sim_info_cache.clone();
        let reader_idx = cfg.reader_index;
        let pump_client = client.clone();
        tokio::spawn(async move {
            let mut event_counter: u32 = 0;
            // Seed sim_info cache at startup
            {
                let mut cache = s_cache.write().await;
                refresh_sim_info_cache(&mut cache, reader_idx).await;
            }
            // Query initial registration state for the volte_ims endpoint.
            tokio::time::sleep(Duration::from_secs(3)).await;
            if let Ok(resp) = pump_client
                .action(vec![("Action", "PJSIPShowEndpoint"), ("Endpoint", "volte_ims")])
                .await
            {
                log::debug!("[ami {label}] PJSIPShowEndpoint response: {resp:?}");
                if let Some(status) = parse_endpoint_status(&resp) {
                    *reg_state.write().await = Some(status);
                }
            }
            loop {
                match rx.recv().await {
                    Ok(pkt) => {
                        event_counter = event_counter.wrapping_add(1);
                        log::trace!(
                            "[ami {label}] event pkt: event={:?} fields={:?}",
                            pkt.event_name(),
                            pkt.fields
                        );
                        // Track registration state from unsolicited AMI events.
                        if let Some(status) = parse_registration_event(&pkt) {
                            *reg_state.write().await = Some(status);
                        }
                        // Refresh sim_info cache from sim_inventory.db every ~60 events.
                        if event_counter % 60 == 0 {
                            let mut cache = s_cache.write().await;
                            refresh_sim_info_cache(&mut cache, reader_idx).await;
                        }
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
            client: client.clone(),
            _shutdown_tx: sd_tx,
            pending_originates,
            registration_state,
            sim_info_cache,
            pending_phone,
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
        Ok(self.sim_info_cache.read().await.clone())
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
        // Route through the [from-sip] dialplan context via a Local channel,
        // same path as when extension 6000 dials a number.  The dialplan emits
        // CallStarted/CallEnded UserEvents and handles recording + Dial().
        let channel = format!("Local/{to}@from-sip");
        let resp = self
            .client
            .action(vec![
                ("Action", "Originate"),
                ("Channel", &channel),
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
        // Remember the phone number so hangup_call can find the active channel.
        {
            let mut p = self.pending_phone.lock().await;
            *p = Some(to.to_string());
        }
        // Return a placeholder call_id; the real call_id (Asterisk UNIQUEID)
        // arrives via the CallStarted UserEvent from the dialplan.
        let placeholder = Uuid::new_v4().to_string();
        Ok(placeholder)
    }

    async fn answer_call(&self) -> Result<()> {
        // Auto-answer is handled by the dialplan (Answer() + MixMonitor()).
        // Manual answer would require AMI Redirect to a dialplan extension;
        // not used in the current product flow.
        Ok(())
    }

    async fn hangup_call(&self, _call_id: Option<&str>) -> Result<()> {
        // Retrieve the phone number from the last originate_call.
        let phone = {
            let p = self.pending_phone.lock().await;
            p.clone()
        };
        let Some(phone) = phone else {
            return Err(anyhow!("No active outbound call to hangup"));
        };

        // Find active PJSIP/volte_ims channel with calleridnum matching phone.
        let resp = self
            .client
            .action_with_timeout(
                vec![
                    ("Action", "Command"),
                    ("Command", &format!("core show channels concise")),
                ],
                Duration::from_secs(5),
            )
            .await
            .context("CoreShowChannels command")?;
        let output = resp.get("output").unwrap_or("");

        // Parse concise format: Channel!Context!Extension!Priority!State!...
        for line in output.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with("Asterisk ") {
                continue;
            }
            let fields: Vec<&str> = trimmed.split('!').collect();
            if fields.len() < 3 {
                continue;
            }
            let chan_name = fields[0];
            let context = fields[1];
            let exten = fields[2];
            // Match a Local channel in from-sip context for this phone.
            // Hanging up the Local channel terminates bridged PJSIP channels too.
            if exten == phone && context == "from-sip" && chan_name.starts_with("Local/") {
                let _ = tokio::time::timeout(
                    Duration::from_secs(5),
                    self.client.action(vec![
                        ("Action", "Hangup"),
                        ("Channel", chan_name),
                    ]),
                )
                .await
                .map_err(|_| anyhow!("Hangup timed out"))??;
                info!("[ami {}] hung up channel {}", self.cfg.ami.label, chan_name);
                // Clear pending phone so we don't double-hangup
                {
                    let mut p = self.pending_phone.lock().await;
                    *p = None;
                }
                return Ok(());
            }
        }
        Err(anyhow!("No active channel found for phone {phone}"))
    }

    async fn read_sms(&self, _: SmsType) -> Result<Vec<ModemSMS>> {
        // AMI delivers SMS via the MessageReceived event; nothing to drain.
        Ok(vec![])
    }

    async fn imei(&self) -> Result<Option<String>> {
        match sim_inventory::get_imei(self.cfg.reader_index) {
            Ok(Some(imei)) if !imei.is_empty() => Ok(Some(imei)),
            _ => Ok(None),
        }
    }

    async fn sim_status(&self) -> Result<Option<String>> {
        let has_sim = sim_inventory::get_mcc_mnc(self.cfg.reader_index)
            .ok()
            .flatten()
            .is_some();
        if has_sim {
            Ok(Some("READY".to_string()))
        } else {
            Ok(Some("ABSENT".to_string()))
        }
    }

    async fn sms_center(&self) -> Result<Option<String>> {
        let cache = self.sim_info_cache.read().await;
        Ok(cache.sms_center.clone())
    }

    async fn registration_status(&self) -> Result<Option<NetworkRegistrationStatus>> {
        Ok(self.registration_state.read().await.clone())
    }

    async fn operator_info(&self) -> Result<Option<OperatorInfo>> {
        let sim = self.sim_info_cache.read().await;
        let mcc = match &sim.mcc {
            Some(m) if !m.is_empty() => m.clone(),
            _ => return Ok(None),
        };
        let mnc = match &sim.mnc {
            Some(n) if !n.is_empty() => n.clone(),
            _ => return Ok(None),
        };
        let name = sim_inventory::lookup_operator(&mcc, &mnc)
            .unwrap_or("Unknown")
            .to_string();
        Ok(Some(OperatorInfo {
            operator_name: name,
            operator_id: format!("{}{}", mcc, mnc),
            registration_status: "home".to_string(),
        }))
    }
}

/// Check if AMI event carries registration state update for the VoLTE endpoint.
fn parse_registration_event(pkt: &AmiPacket) -> Option<NetworkRegistrationStatus> {
    let event = pkt.event_name()?;
    match event {
        "ContactStatus" => {
            // Unsolicited ContactStatus: endpointname = registration/endpoint name
            let endpoint = pkt.get("endpointname")?;
            let aor = pkt.get("aor").unwrap_or("");
            if endpoint != "volte_ims" && aor != "volte_ims" {
                return None;
            }
            let status = pkt.get("contactstatus").unwrap_or("");
            let registered = status.eq_ignore_ascii_case("Reachable")
                || status.eq_ignore_ascii_case("Registered")
                || status.eq_ignore_ascii_case("Online")
                || status.eq_ignore_ascii_case("NonQualified");
            Some(NetworkRegistrationStatus {
                status: if registered { "1" } else { "0" }.into(),
                location_area_code: None,
                cell_id: None,
            })
        }
        "RegistrationStatus" => {
            // Response to PJSIPShowRegistrationInboundContactStatus
            let endpoint = pkt.get("endpointname").or_else(|| pkt.get("registration"))?;
            if endpoint != "volte_ims" {
                return None;
            }
            let status = pkt.get("status")?;
            let registered = status.eq_ignore_ascii_case("Registered")
                || status.eq_ignore_ascii_case("Reachable")
                || status.eq_ignore_ascii_case("Online");
            Some(NetworkRegistrationStatus {
                status: if registered { "1" } else { "0" }.into(),
                location_area_code: None,
                cell_id: None,
            })
        }
        "PeerStatus" => {
            // PeerStatus for PJSIP registrations
            let peer = pkt.get("peer").unwrap_or("");
            if peer != "PJSIP/volte_ims" && peer != "volte_ims" {
                return None;
            }
            let status = pkt.get("peerstatus")?;
            let registered = status.eq_ignore_ascii_case("Reachable")
                || status.eq_ignore_ascii_case("Registered")
                || status.eq_ignore_ascii_case("Online");
            Some(NetworkRegistrationStatus {
                status: if registered { "1" } else { "0" }.into(),
                location_area_code: None,
                cell_id: None,
            })
        }
        _ => None,
    }
}

/// Parse PJSIPShowEndpoint response into registration status.
fn parse_endpoint_status(pkt: &AmiPacket) -> Option<NetworkRegistrationStatus> {
    // The action() response for PJSIPShowEndpoint is just "Response: Success".
    // We rely on the ContactDetail/EndpointDetail events that follow on the
    // broadcast channel instead. This is a fallback that returns None.
    // Registration status is captured from ContactDetail events in the event pump.
    if pkt.is_success() {
        // Mark as registered (code 1 = home network) since the endpoint
        // exists and is configured. Use 3GPP TS 27.007 registration codes
        // so the frontend displays the correct i18n label.
        Some(NetworkRegistrationStatus {
            status: "1".to_string(),
            location_area_code: None,
            cell_id: None,
        })
    } else {
        None
    }
}

/// Periodically refresh SIM identity cache from sim_inventory.db.
/// Uses a simple counter to limit DB reads to once per ~60 events.
async fn refresh_sim_info_cache(cache: &mut super::SimInfo, reader_index: u8) {
    if let Ok(Some(msisdn)) = sim_inventory::get_msisdn(reader_index) {
        cache.msisdn = Some(msisdn);
    }
    if let Ok(Some((mcc, mnc))) = sim_inventory::get_mcc_mnc(reader_index) {
        cache.mcc = Some(mcc);
        cache.mnc = Some(mnc);
    }
    if let Ok(Some(sms_center)) = sim_inventory::get_sms_center(reader_index) {
        cache.sms_center = Some(sms_center);
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
                direction: "inbound".to_string(),
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
            let direction = pkt
                .get("direction")
                .filter(|s| !s.is_empty())
                .unwrap_or("inbound")
                .to_string();
            Some(ModemEvent::CallRinging {
                sim_id: sim_id.to_string(),
                call_id,
                phone,
                direction,
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
