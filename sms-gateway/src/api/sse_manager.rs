use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

use crate::db::Conversation;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallEvent {
    pub event_type: String, // "incoming_call" | "outbound_call" | "call_ended" | "call_answered"
    pub sim_id: String,
    pub call_id: String,
    pub phone: Option<String>,
    pub direction: String,
}

#[derive(Clone)]
pub struct SseManager {
    tx: broadcast::Sender<Vec<Conversation>>,
    call_tx: broadcast::Sender<CallEvent>,
}

impl Default for SseManager {
    fn default() -> Self {
        Self::new()
    }
}

impl SseManager {
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(100);
        let (call_tx, _) = broadcast::channel(100);
        Self { tx, call_tx }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Vec<Conversation>> {
        self.tx.subscribe()
    }

    pub fn subscribe_calls(&self) -> broadcast::Receiver<CallEvent> {
        self.call_tx.subscribe()
    }

    pub fn send(&self, msg: Vec<Conversation>) {
        let _ = self.tx.send(msg);
    }

    pub fn send_call_event(&self, event: CallEvent) {
        let _ = self.call_tx.send(event);
    }
}
