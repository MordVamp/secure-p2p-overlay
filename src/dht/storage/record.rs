//! dht/storage/record.rs — DHT-запись с TTL и подписью.

use serde::{Deserialize, Serialize};
use crate::types::{NodeId, now_ms};

/// Ключ записи — SHA-256 от ключевой строки (32 байта).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RecordKey(pub [u8; 32]);

impl RecordKey {
    pub fn from_str(s: &str) -> Self {
        use sha2::{Sha256, Digest};
        let mut h = Sha256::new();
        h.update(s.as_bytes());
        RecordKey(h.finalize().into())
    }

    pub fn to_hex(&self) -> String { hex::encode(self.0) }
}

impl From<&NodeId> for RecordKey {
    fn from(id: &NodeId) -> Self { RecordKey(id.0) }
}

/// DHT-запись: значение + метаданные.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DhtRecord {
    pub key:         RecordKey,
    pub value:       Vec<u8>,
    /// Публикующий узел
    pub publisher:   NodeId,
    /// Время публикации (unix ms)
    pub published_ms: u64,
    /// TTL в секундах
    pub ttl_seconds: u64,
    /// Ed25519 подпись publisher'а над (key || value || published_ms) — для Фазы 4+
    pub signature:   Option<Vec<u8>>,
}

impl DhtRecord {
    pub fn new(key: RecordKey, value: Vec<u8>, publisher: NodeId, ttl_seconds: u64) -> Self {
        Self {
            key, value, publisher,
            published_ms: now_ms(),
            ttl_seconds,
            signature: None,
        }
    }

    /// Истёк ли TTL?
    pub fn is_expired(&self) -> bool {
        let age_ms = now_ms().saturating_sub(self.published_ms);
        age_ms > self.ttl_seconds * 1000
    }

    /// Оставшееся время жизни в секундах.
    pub fn remaining_ttl_secs(&self) -> u64 {
        let age_ms = now_ms().saturating_sub(self.published_ms);
        let age_sec = age_ms / 1000;
        self.ttl_seconds.saturating_sub(age_sec)
    }
}
