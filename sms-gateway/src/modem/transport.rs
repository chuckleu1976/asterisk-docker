//! Abstract transport layer for a single SIM / IMS endpoint.
//!
//! Current implementation:
//!   * `ami` — talks to an asterisk container over the AMI socket; carrier
//!     events (SMS, calls) arrive as native AMI events.
//!
//! The legacy serial / AT-command backend was removed in Phase B step 5; the
//! trait is preserved as a slim abstraction so additional backends can be
//! added without touching `ModemManager`. Diagnostic getters a backend cannot
//! answer return `Ok(None)`.

use std::path::PathBuf;

use async_trait::async_trait;
use tokio::sync::mpsc;

use crate::config::SmsStorage;
use crate::db::ModemSMS;

use super::types::{
    ModemInfo, NetworkRegistrationStatus, OperatorInfo, SmsType,
};

/// Identity data we can read for the SIM attached to a transport.
#[derive(Debug, Clone, Default)]
pub struct SimInfo {
    pub iccid: Option<String>,
    pub imsi: Option<String>,
    pub msisdn: Option<String>,
    pub mcc: Option<String>,
    pub mnc: Option<String>,
    pub sms_center: Option<String>,
}

/// Events emitted by a transport. The owner (ModemManager) fans these out
/// to the DB, SSE, webhooks, and whisper transcription pipeline.
#[derive(Debug, Clone)]
pub enum ModemEvent {
    /// Incoming SMS, already decoded.
    SmsReceived {
        sim_id: String,
        from: String,
        body: String,
        timestamp: chrono::DateTime<chrono::Utc>,
    },
    /// Call started (inbound ringing, or outbound originated from another SIP client).
    CallRinging {
        sim_id: String,
        call_id: String,
        phone: String,
        /// "inbound" or "outbound"
        direction: String,
    },
    /// Outbound or inbound call answered.
    CallAnswered {
        sim_id: String,
        call_id: String,
    },
    /// Call torn down.
    CallEnded {
        sim_id: String,
        call_id: String,
        /// Present when the backend produced a recording for this call.
        recording_path: Option<PathBuf>,
    },
    /// A recording file finished writing and is ready for transcription.
    RecordingDone {
        sim_id: String,
        call_id: String,
        path: PathBuf,
    },
}

pub type EventRx = mpsc::Receiver<ModemEvent>;
pub type EventTx = mpsc::Sender<ModemEvent>;

/// Abstract transport for one SIM / IMS endpoint.
#[async_trait]
pub trait Transport: Send + Sync {
    /// Stable identifier for this transport (ICCID once known; fallback id otherwise).
    fn sim_id(&self) -> &str;

    /// Human-readable label, e.g. `asterisk2@127.0.0.1:5039` or `/dev/ttyUSB2`.
    fn label(&self) -> &str;

    /// Identity data for the attached SIM.
    async fn sim_info(&self) -> anyhow::Result<SimInfo>;

    // ─── Messaging ────────────────────────────────────────────────────────

    /// Send an SMS to `to` containing `body`.
    async fn send_sms(&self, to: &str, body: &str) -> anyhow::Result<()>;

    /// Drain queued/stored SMS messages (serial path). AMI transports rely on
    /// pushed events instead and return `Ok(vec![])`.
    async fn read_sms(&self, _sms_type: SmsType) -> anyhow::Result<Vec<ModemSMS>> {
        Ok(vec![])
    }

    // ─── Voice ────────────────────────────────────────────────────────────

    /// Originate an outbound call. Returns a transport-specific call id
    /// (AMI channel name, or an ATD lock token for serial).
    async fn originate_call(&self, to: &str) -> anyhow::Result<String>;

    /// Answer the currently ringing inbound call, if any.
    async fn answer_call(&self) -> anyhow::Result<()>;

    /// Hang up. `call_id` selects a specific channel for AMI; ignored by serial.
    async fn hangup_call(&self, call_id: Option<&str>) -> anyhow::Result<()>;

    // ─── Diagnostics (Ok(None) when not applicable) ───────────────────────

    async fn registration_status(&self) -> anyhow::Result<Option<NetworkRegistrationStatus>> { Ok(None) }
    async fn operator_info(&self) -> anyhow::Result<Option<OperatorInfo>> { Ok(None) }
    async fn modem_model(&self) -> anyhow::Result<Option<ModemInfo>> { Ok(None) }
    async fn imei(&self) -> anyhow::Result<Option<String>> { Ok(None) }
    async fn sms_center(&self) -> anyhow::Result<Option<String>> { Ok(None) }
    async fn sim_status(&self) -> anyhow::Result<Option<String>> { Ok(None) }
    async fn memory_status(&self) -> anyhow::Result<Option<String>> { Ok(None) }
    async fn temperature_info(&self) -> anyhow::Result<Option<String>> { Ok(None) }
    async fn network_info(&self) -> anyhow::Result<Option<String>> { Ok(None) }
    async fn sms_storage_status(&self) -> anyhow::Result<Option<String>> { Ok(None) }
    async fn set_sms_storage(&self, _storage: SmsStorage) -> anyhow::Result<()> { Ok(()) }
}
