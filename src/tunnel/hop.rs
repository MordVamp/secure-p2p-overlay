//! tunnel/hop.rs — Один промежуточный узел (relay) в маршруте туннеля.

use serde::{Deserialize, Serialize};
use crate::types::Contact;

/// Описание одного hop-а в маршруте туннеля.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TunnelHop {
    /// Контакт relay-узла.
    pub relay: Contact,
    /// Индекс hop в маршруте (0 = вход, last = выход).
    pub hop_index: usize,
    /// Последний успешный ACK (unix ms). 0 = не было.
    pub last_ack_ms: u64,
    /// Флаг: relay подтвердил участие в туннеле.
    pub confirmed: bool,
}

impl TunnelHop {
    pub fn new(relay: Contact, hop_index: usize) -> Self {
        Self { relay, hop_index, last_ack_ms: 0, confirmed: false }
    }

    pub fn mark_confirmed(&mut self) {
        use crate::types::now_ms;
        self.last_ack_ms = now_ms();
        self.confirmed = true;
    }

    pub fn is_alive(&self, ack_timeout_ms: u64) -> bool {
        if !self.confirmed { return false; }
        if self.last_ack_ms == 0 { return false; }
        use crate::types::now_ms;
        now_ms() - self.last_ack_ms < ack_timeout_ms
    }
}
