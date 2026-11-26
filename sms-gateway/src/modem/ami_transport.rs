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

use super::ami::{AmiClient, AmiConfig, AmiPacket};
use super::transport::{ModemEvent, SimInfo, Transport};
use super::types::{NetworkRegistrationStatus, OperatorInfo, SmsType};
use crate::db::ModemSMS;
use crate::sim_inventory;

/// PJSIP endpoint/registration name configured in `config/<N>/asterisk/pjsip.conf`.
const VOLTE_ENDPOINT: &str = "volte_ims";

/// Default IMS domain used when the device config does not provide one.
const DEFAULT_IMS_DOMAIN: &str = "ims.mnc240.mcc310.3gppnetwork.org";

fn valid_phone(phone: &str) -> bool {
    let digits = phone.strip_prefix('+').unwrap_or(phone);
    (3..=15).contains(&digits.len()) && digits.bytes().all(|value| value.is_ascii_digit())
}

#[derive(Debug)]
struct ActiveChannel {
    name: String,
    context: String,
    state: String,
}

fn parse_channels(output: &str) -> Vec<ActiveChannel> {
    output.lines().filter_map(|line| {
        let fields: Vec<_> = line.trim().split('!').collect();
        if fields.len() < 6 || !fields[0].contains('/') { return None; }
        Some(ActiveChannel { name: fields[0].to_owned(), context: fields[1].to_owned(), state: fields[4].to_owned() })
    }).collect()
}

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
    /// IMS domain used for outbound SIP MESSAGE (e.g. ims.mnc240.mcc310.3gppnetwork.org).
    pub ims_domain: Option<String>,
}

pub struct AmiTransport {
    cfg: AmiTransportConfig,
    client: std::sync::Arc<AmiClient>,
    /// Optional handle to the supervisor shutdown channel.
    _shutdown_tx: mpsc::Sender<()>,
    /// Cached registration state, updated by RegistrationStatus AMI events.
    registration_state: Arc<RwLock<Option<NetworkRegistrationStatus>>>,
    /// Cached SIM identity, refreshed periodically from sim_inventory.db.
    sim_info_cache: Arc<RwLock<super::SimInfo>>,
    pending_call: Arc<Mutex<Option<String>>>,
}

impl AmiTransport {
    async fn active_channels(&self) -> Result<Vec<ActiveChannel>> {
        let response = self.client.action_with_timeout(
            vec![("Action", "Command"), ("Command", "core show channels concise")],
            Duration::from_secs(5),
        ).await?;
        anyhow::ensure!(response.is_success() || response.response() == Some("Follows"), "Unable to inspect call channels");
        Ok(parse_channels(response.get("output").unwrap_or("")))
    }

    /// Spawn the AMI client supervisor and an event-pump task that forwards
    /// translated events to `events_tx`.
    pub fn spawn(cfg: AmiTransportConfig, events_tx: mpsc::Sender<ModemEvent>) -> Self {
        let (sd_tx, sd_rx) = mpsc::channel::<()>(1);
        let client = std::sync::Arc::new(AmiClient::spawn(cfg.ami.clone(), sd_rx));
        let pending_originates = Arc::new(Mutex::new(HashMap::new()));
        let registration_state: Arc<RwLock<Option<NetworkRegistrationStatus>>> =
            Arc::new(RwLock::new(None));
        let sim_info_cache = Arc::new(RwLock::new(cfg.sim_info.clone()));
        let pending_call = Arc::new(Mutex::new(None));

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
            let _ = pump_client
                .action(vec![("Action", "PJSIPShowEndpoint"), ("Endpoint", "volte_ims")])
                .await;
            // Query outbound registration status — generates OutboundRegistrationDetail
            // events that the event pump captures via parse_registration_event.
            let _ = pump_client
                .action(vec![("Action", "PJSIPShowRegistrationsOutbound")])
                .await;
            let mut reg_refresh = tokio::time::interval(Duration::from_secs(30));
            // Skip the immediate tick; first refresh happens after 30 s.
            reg_refresh.tick().await;
            loop {
                tokio::select! {
                    biased;
                    _ = reg_refresh.tick() => {
                        // Re-query outbound registration status so that a state
                        // change after startup (e.g. Registered → Rejected) does
                        // not remain stale in the frontend.
                        let _ = pump_client
                            .action(vec![("Action", "PJSIPShowRegistrationsOutbound")])
                            .await;
                    }
                    result = rx.recv() => {
                        match result {
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
                }
            }
        });

        Self {
            cfg,
            client: client.clone(),
            _shutdown_tx: sd_tx,
            registration_state,
            sim_info_cache,
            pending_call,
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
        //   To: pjsip:volte_ims/sip:+<num>@<ims_domain>
        //   From: <endpoint name>
        //   Body: <text>
        // The slash splits endpoint from Request-URI; the domain must be the
        // real IMS domain (not the endpoint name) so Asterisk can resolve it.
        let ims_domain = self
            .cfg
            .ims_domain
            .as_deref()
            .unwrap_or(DEFAULT_IMS_DOMAIN);
        let to_uri = format!("pjsip:{VOLTE_ENDPOINT}/sip:{to}@{ims_domain}");
        use base64::Engine;
        let encoded = base64::engine::general_purpose::STANDARD.encode(body);
        log::debug!("[ami {}] submitting SMS", self.cfg.ami.label);
        let resp = self
            .client
            .action(vec![
                ("Action", "MessageSend"),
                ("To", &to_uri),
                ("From", VOLTE_ENDPOINT),
                ("Base64Body", &encoded),
            ])
            .await
            .context("MessageSend action")?;
        if !resp.is_success() {
            log::error!(
                "[ami {}] MessageSend failed: {:?}",
                self.cfg.sim_id, resp
            );
            return Err(anyhow!(
                "MessageSend failed: {}",
                resp.message().unwrap_or("(no message)")
            ));
        }
        log::info!(
            "[ami {}] MessageSend succeeded: {:?}",
            self.cfg.sim_id, resp
        );
        Ok(())
    }

    async fn originate_call(&self, to: &str, call_id: &str) -> Result<String> {
        anyhow::ensure!(valid_phone(to), "Invalid telephone number");
        anyhow::ensure!(self.active_channels().await?.is_empty(), "This line already has a call");
        let peer_id = format!("{call_id}-peer");
        let variable = format!("__GATEWAY_CALL_ID={call_id}");
        let channel = format!("Local/{to}@from-sip/n");
        let resp = self
            .client
            .action_with_id(vec![
                ("Action", "Originate"),
                ("Channel", &channel),
                ("ChannelId", &call_id),
                ("OtherChannelId", &peer_id),
                ("Variable", &variable),
                ("Application", "Wait"),
                ("Data", "3600"),
                ("Timeout", "30000"),
                ("Async", "true"),
            ], call_id)
            .await
            .context("Originate action")?;
        if !resp.is_success() {
            return Err(anyhow!(
                "Originate failed: {}",
                resp.message().unwrap_or("(no message)")
            ));
        }
        *self.pending_call.lock().await = Some(call_id.to_owned());
        Ok(call_id.to_owned())
    }

    async fn answer_call(&self) -> Result<()> {
        let channels = self.active_channels().await?;
        let channel = channels.iter().find(|channel| {
            channel.context == "gateway-ringing" && matches!(channel.state.as_str(), "Ring" | "Ringing")
        }).ok_or_else(|| anyhow!("No ringing incoming call"))?;
        let response = self.client.action(vec![
            ("Action", "Redirect"), ("Channel", &channel.name),
            ("Context", "gateway-answer"), ("Exten", "s"), ("Priority", "1"),
        ]).await?;
        anyhow::ensure!(response.is_success(), "Answer failed: {}", response.message().unwrap_or("AMI rejected redirect"));
        Ok(())
    }

    async fn hangup_call(&self, call_id: Option<&str>) -> Result<()> {
        let channels = self.active_channels().await?;
        for channel in channels {
            if let Some(wanted) = call_id {
                let value = self.client.action(vec![
                    ("Action", "Getvar"), ("Channel", &channel.name), ("Variable", "GATEWAY_CALL_ID"),
                ]).await?;
                if value.get("value") != Some(wanted) { continue; }
            }
            let response = self.client.action(vec![("Action", "Hangup"), ("Channel", &channel.name)]).await?;
            if response.is_success() {
                *self.pending_call.lock().await = None;
                return Ok(());
            }
            anyhow::ensure!(matches!(response.message(), Some("No such channel" | "Channel does not exist")),
                "Hangup failed: {}", response.message().unwrap_or("AMI rejected hangup"));
        }
        *self.pending_call.lock().await = None;
        Ok(())
    }

    async fn call_mode(&self) -> Result<String> {
        let response = self.client.action(vec![("Action", "Command"), ("Command", "database get gateway mode")]).await?;
        let output = response.get("output").unwrap_or("");
        if output.lines().any(|line| line.trim() == "Value: plain") { return Ok("plain".into()); }
        anyhow::ensure!(output.contains("Database entry not found") || output.contains("Value: telegram"), "Unable to read call routing");
        Ok("telegram".into())
    }

    async fn set_call_mode(&self, mode: &str) -> Result<()> {
        anyhow::ensure!(matches!(mode, "plain" | "telegram"), "Invalid call mode");
        anyhow::ensure!(self.active_channels().await?.is_empty(), "Cannot change routing during a call");
        let dialplan = self.client.action(vec![("Action", "Command"), ("Command", "dialplan show gateway-answer")]).await?;
        anyhow::ensure!(dialplan.get("output").is_some_and(|output| output.contains("Context 'gateway-answer'")), "Plain-call dialplan is not installed");
        let command = format!("database put gateway mode {mode}");
        let response = self.client.action(vec![("Action", "Command"), ("Command", &command)]).await?;
        anyhow::ensure!(response.get("output").is_some_and(|output| output.contains("Updated database successfully")), "Unable to save call routing");
        Ok(())
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
        let reg = self.registration_state.read().await;
        let registration_status = match reg.as_ref() {
            Some(s) if s.status == "1" => "home".into(),
            Some(_) => "rejected".into(),
            None => "unknown".into(),
        };
        Ok(Some(OperatorInfo {
            operator_name: name,
            operator_id: format!("{}{}", mcc, mnc),
            registration_status,
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
        "OutboundRegistrationDetail" => {
            // Response to PJSIPShowRegistrationsOutbound
            let name = pkt.get("objectname").or_else(|| pkt.get("endpoint"))?;
            if name != "volte_ims" {
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
        _ => None,
    }
}

/// Periodically refresh SIM identity cache from sim_inventory.db.
/// Uses a simple counter to limit DB reads to once per ~60 events.
async fn refresh_sim_info_cache(cache: &mut super::SimInfo, reader_index: u8) {
    match sim_inventory::get_iccid(reader_index) {
        Ok(Some(iccid)) => cache.iccid = Some(iccid),
        Ok(None) => cache.iccid = None,
        _ => {}
    }
    match sim_inventory::get_imsi(reader_index) {
        Ok(Some(imsi)) => cache.imsi = Some(imsi),
        Ok(None) => cache.imsi = None,
        _ => {}
    }
    match sim_inventory::get_msisdn(reader_index) {
        Ok(Some(msisdn)) => cache.msisdn = Some(msisdn),
        Ok(None) => cache.msisdn = None,
        _ => {}
    }
    match sim_inventory::get_mcc_mnc(reader_index) {
        Ok(Some((mcc, mnc))) => {
            cache.mcc = Some(mcc);
            cache.mnc = Some(mnc);
        }
        Ok(None) => {
            cache.mcc = None;
            cache.mnc = None;
        }
        _ => {}
    }
    match sim_inventory::get_sms_center(reader_index) {
        Ok(Some(sms_center)) => cache.sms_center = Some(sms_center),
        Ok(None) => cache.sms_center = None,
        _ => {}
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
        "UserEvent" => {
            let me = translate_user_event(pkt, sim_id);
            // Save recording_path → call_id so MixMonitorStop can correlate.
            if let Some(ModemEvent::CallEnded { ref call_id, recording_path: Some(ref rp), .. }) = me {
                let key = rp.to_string_lossy().into_owned();
                let mut map = pending_originates.lock().await;
                debug!("[ami {sim_id}] CallEnded recording_path={key} call_id={call_id}");
                map.insert(key, call_id.clone());
            }
            me
        }
        "MessageReceived" => {
            let callerid = pkt.get("calleridnum").unwrap_or("").to_string();
            let from = pkt.get("from").unwrap_or("").to_string();
            let raw_body = pkt.get("body").unwrap_or("").to_string();
            let body = sanitize_incoming_sms_body(&raw_body, sim_id, "MessageReceived");
            // Prefer calleridnum (Asterisk's parsed caller ID) over the raw
            // SIP From header — the IMS network may replace From with a trunk
            // identifier (e.g. "17530") while calleridnum holds the E.164 number.
            let from = if callerid.is_empty() { from } else { callerid };
            if from.is_empty() && body.is_empty() {
                return None;
            }
            debug!("[ami {sim_id}] MessageReceived from={from} body={body}");
            Some(ModemEvent::SmsReceived {
                sim_id: sim_id.to_string(),
                from,
                body,
                timestamp: chrono::Utc::now(),
            })
        }
        // Inbound call ringing
        "Newchannel" => {
            None
        }
        // Call answered (state moved to Up)
        "Newstate" => {
            let state = pkt.get("channelstatedesc").unwrap_or("");
            if !state.eq_ignore_ascii_case("Up") {
                return None;
            }
            let channel = pkt.get("channel")?;
            if !channel.starts_with("PJSIP/volte_ims-") { return None; }
            let call_id = pkt.get("linkedid").or_else(|| pkt.get("uniqueid"))?.to_string();
            Some(ModemEvent::CallAnswered {
                sim_id: sim_id.to_string(),
                call_id,
            })
        }
        "Hangup" => {
            let channel = pkt.get("channel")?;
            if !channel.starts_with("PJSIP/volte_ims-") { return None; }
            let call_id = pkt.get("linkedid").or_else(|| pkt.get("uniqueid"))?.to_string();
            Some(ModemEvent::CallEnded {
                sim_id: sim_id.to_string(),
                call_id,
                recording_path: None,
            })
        }
        "OriginateResponse" => {
            if pkt.get("response").is_some_and(|value| value.eq_ignore_ascii_case("Success")) { return None; }
            Some(ModemEvent::CallEnded {
                sim_id: sim_id.to_owned(), call_id: pkt.action_id()?.to_owned(), recording_path: None,
            })
        }
        // MixMonitor finished writing a recording file.
        "MixMonitorStop" => {
            let file = pkt.get("file")?.to_string();
            let path = PathBuf::from(&file);
            let call_id = pending_originates
                .lock()
                .await
                .remove(&file)
                .unwrap_or_else(|| {
                    debug!("[ami {sim_id}] MixMonitorStop no pending entry for {file}");
                    file.clone()
                });
            debug!("[ami {sim_id}] MixMonitorStop file={file} call_id={call_id}");
            Some(ModemEvent::RecordingDone {
                sim_id: sim_id.to_string(),
                call_id,
                path,
            })
        }
        _ => {
            debug!("[ami {sim_id}] unhandled event: {}", name);
            None
        }
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
            let raw_body = pkt.get("body").unwrap_or("").to_string();
            let body = sanitize_incoming_sms_body(&raw_body, sim_id, "UserEvent/SmsReceived");
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

fn sanitize_incoming_sms_body(body: &str, sim_id: &str, source: &str) -> String {
    if body.is_empty() {
        return String::new();
    }

    if is_likely_binary_or_mojibake(body) {
        warn!(
            "[ami {}] {} body looks non-text; replacing with marker (len={}, utf8_hex={})",
            sim_id,
            source,
            body.len(),
            hex::encode(body.as_bytes())
        );
        return format!("[BINARY/UNSUPPORTED SMS] ({} bytes)", body.len());
    }

    body.to_string()
}

fn is_likely_binary_or_mojibake(body: &str) -> bool {
    let trimmed = body.trim();
    if trimmed.is_empty() {
        return false;
    }

    // Allow common non-Latin scripts to pass through untouched.
    if contains_cjk(trimmed) {
        return false;
    }

    let mut total = 0usize;
    let mut suspicious = 0usize;
    let mut controls = 0usize;
    let mut question_marks = 0usize;
    let mut ascii_word = 0usize;
    let mut non_ascii = 0usize;

    for ch in trimmed.chars() {
        total += 1;
        if ch == '?' {
            question_marks += 1;
        }
        if ch.is_ascii_alphanumeric() {
            ascii_word += 1;
        } else {
            non_ascii += 1;
        }
        if ch.is_control() && ch != '\n' && ch != '\r' && ch != '\t' {
            controls += 1;
            continue;
        }
        if ch == '\u{FFFD}' || is_gsm_mojibake_char(ch) {
            suspicious += 1;
        }
    }

    if controls > 0 {
        return true;
    }
    if total < 8 {
        return false;
    }

    let suspicious_ratio = suspicious as f32 / total as f32;
    let question_ratio = question_marks as f32 / total as f32;
    let non_ascii_ratio = non_ascii as f32 / total as f32;

    (suspicious >= 4 && suspicious_ratio >= 0.18)
        || (suspicious >= 3 && question_ratio >= 0.12 && ascii_word >= 3)
        || (question_marks >= 2 && suspicious >= 2 && non_ascii_ratio >= 0.08 && ascii_word >= 4)
}

fn is_gsm_mojibake_char(ch: char) -> bool {
    matches!(
        ch as u32,
        0x00A3
            | 0x00A5
            | 0x00A8
            | 0x00A9
            | 0x00AA
            | 0x00B0
            | 0x00C5
            | 0x00C6
            | 0x00C7
            | 0x00C8
            | 0x00D8
            | 0x00DF
            | 0x00E0
            | 0x00E5
            | 0x00E6
            | 0x00E8
            | 0x00E9
            | 0x00EC
            | 0x00F2
            | 0x00F8
            | 0x00F9
            | 0x0393
            | 0x0394
            | 0x0398
            | 0x039B
            | 0x039E
            | 0x03A0
            | 0x03A3
            | 0x03A6
            | 0x03A8
            | 0x03A9
    )
}

fn contains_cjk(s: &str) -> bool {
    s.chars().any(|c| {
        let cp = c as u32;
        (0x4E00..=0x9FFF).contains(&cp)
            || (0x3400..=0x4DBF).contains(&cp)
            || (0x3040..=0x30FF).contains(&cp)
            || (0xAC00..=0xD7AF).contains(&cp)
    })
}

#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn originate_failure_uses_the_queued_call_id() {
        let packet = super::AmiPacket { fields: [
            ("event".into(), "OriginateResponse".into()),
            ("response".into(), "Failure".into()),
            ("actionid".into(), "test-call-id".into()),
        ].into_iter().collect() };
        let pending = std::sync::Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new()));
        let event = super::translate_event(&packet, "line-a", &pending).await.unwrap();
        assert!(matches!(event, super::ModemEvent::CallEnded { call_id, .. } if call_id == "test-call-id"));
    }

    #[test]
    fn call_inputs_and_channel_selection_are_bounded() {
        assert!(super::valid_phone("+12025550123"));
        assert!(super::valid_phone("02025550123"));
        for phone in ["", "12", "1\r\nAction: Hangup", "1@other", "1234567890123456"] {
            assert!(!super::valid_phone(phone));
        }
        let channels = super::parse_channels("PJSIP/volte_ims-0001!volte_ims!s!1!Ring!Wait!30\nLocal/test;1!from-sip!s!1!Up!Wait!30\n--END COMMAND--");
        assert_eq!(channels.len(), 2);
        assert_eq!(channels[0].name, "PJSIP/volte_ims-0001");
        assert_eq!(channels[0].state, "Ring");
        assert_eq!(channels[1].context, "from-sip");
    }

    use super::{is_likely_binary_or_mojibake, sanitize_incoming_sms_body};

    #[test]
    fn plain_ascii_sms_is_kept() {
        let body = "Your OTP is 123456";
        let out = sanitize_incoming_sms_body(body, "sim1", "test");
        assert_eq!(out, body);
    }

    #[test]
    fn cjk_sms_is_kept() {
        let body = "\u{4F60}\u{597D}\u{4E16}\u{754C}";
        let out = sanitize_incoming_sms_body(body, "sim1", "test");
        assert_eq!(out, body);
    }

    #[test]
    fn mojibake_like_sms_is_replaced() {
        let body = "PayPaly\u{039E}??v Ax\u{00A3}f/0383320$??v Ax\u{00A3}\u{00F9}W";
        assert!(is_likely_binary_or_mojibake(body));

        let out = sanitize_incoming_sms_body(body, "sim1", "test");
        assert!(out.starts_with("[BINARY/UNSUPPORTED SMS]"));
    }
}
