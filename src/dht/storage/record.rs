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
    /// A7: публичный ключ публикатора (NodeID = hash(ключ) проверяется в `verify`).
    #[serde(default)]
    pub publisher_key: Vec<u8>,
}

impl DhtRecord {
    pub fn new(key: RecordKey, value: Vec<u8>, publisher: NodeId, ttl_seconds: u64) -> Self {
        Self {
            key, value, publisher,
            published_ms: now_ms(),
            ttl_seconds,
            signature: None,
            publisher_key: Vec::new(),
        }
    }

    /// A7: байты для подписи: домен || key || publisher || value.
    /// TTL и время не подписываются — их ограничивает/назначает принимающий узел.
    pub fn signing_bytes(key: &RecordKey, publisher: &NodeId, value: &[u8]) -> Vec<u8> {
        let mut b = b"P2P-DHT-RECORD-v1".to_vec();
        b.extend_from_slice(&key.0);
        b.extend_from_slice(&publisher.0);
        b.extend_from_slice(value);
        b
    }

    /// A7: создать запись, подписанную идентичностью узла.
    pub fn new_signed(id: &crate::identity::node_identity::NodeIdentity,
                      key: RecordKey, value: Vec<u8>, ttl_seconds: u64) -> Self {
        let sig = id.sign(&Self::signing_bytes(&key, &id.node_id, &value));
        let mut r = Self::new(key, value, id.node_id, ttl_seconds);
        r.publisher_key = id.public_key_bytes().to_vec();
        r.signature = Some(sig.to_bytes().to_vec());
        r
    }

    /// A7: подпись есть, ключ соответствует publisher, подпись валидна.
    pub fn verify(&self) -> bool {
        use crate::identity::node_identity::{verify_node_id_bytes, verify_signature};
        use ed25519_dalek::{Signature, VerifyingKey};
        let Some(sig) = &self.signature else { return false };
        if !verify_node_id_bytes(&self.publisher, &self.publisher_key) { return false; }
        let (Ok(pk), Ok(sb)) = (
            <[u8; 32]>::try_from(self.publisher_key.as_slice()),
            <[u8; 64]>::try_from(sig.as_slice()),
        ) else { return false };
        let Ok(vk) = VerifyingKey::from_bytes(&pk) else { return false };
        verify_signature(&vk, &Self::signing_bytes(&self.key, &self.publisher, &self.value),
                         &Signature::from_bytes(&sb))
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
