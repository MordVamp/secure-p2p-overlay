//! identity/record.rs — Подписанная адресная запись DHT (NodeRecord).
//!
//! NodeRecord хранится в DHT по ключу SHA-256("node:" || NodeID).
//! Перед сохранением и при получении проверяются:
//!   - соответствие node_id открытому ключу
//!   - цифровая подпись
//!   - срок действия (expires_at)
//!   - монотонность sequence_number (защита от rollback)

use serde::{Deserialize, Serialize};
use ed25519_dalek::{Signature, VerifyingKey};
use sha2::{Sha256, Digest};
use std::net::SocketAddr;
use thiserror::Error;

use crate::types::{NodeId, now_unix};
use super::node_identity::{verify_node_id, verify_signature};

/// Ключ DHT для адресной записи узла.
pub fn node_record_key(node_id: &NodeId) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(b"node:");
    h.update(node_id.0);
    h.finalize().into()
}

/// Адресная запись узла, хранимая в DHT.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeRecord {
    pub node_id:          NodeId,
    /// Canonical-encoded Ed25519 pubkey (32 байта).
    pub identity_pubkey:  [u8; 32],
    /// Список адресов, на которых узел принимает соединения.
    pub addresses:        Vec<SocketAddr>,
    /// Монотонно возрастающий порядковый номер (защита от отката).
    pub sequence_number:  u64,
    /// Время создания (Unix timestamp, секунды).
    pub issued_at:        u64,
    /// Время истечения (Unix timestamp, секунды).
    pub expires_at:       u64,
    /// Ed25519-подпись canonical_encode(остальных полей).
    pub signature:        Vec<u8>,

    // РАСШИРЕНИЕ (продвинутый уровень): alias поиск по псевдониму
    // pub alias: Option<String>,
}

impl NodeRecord {
    /// Создать и подписать запись.
    pub fn new(
        node_id: NodeId,
        identity_pubkey: [u8; 32],
        addresses: Vec<SocketAddr>,
        sequence_number: u64,
        ttl_seconds: u64,
        sign_fn: impl Fn(&[u8]) -> Vec<u8>,
    ) -> Self {
        let now = now_unix();
        let mut rec = Self {
            node_id,
            identity_pubkey,
            addresses,
            sequence_number,
            issued_at:  now,
            expires_at: now + ttl_seconds,
            signature:  vec![],
        };
        let payload = rec.canonical_bytes();
        rec.signature = sign_fn(&payload);
        rec
    }

    /// Байты для подписи/верификации (всё кроме самой подписи).
    pub fn canonical_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.extend_from_slice(&self.node_id.0);
        buf.extend_from_slice(&self.identity_pubkey);
        for addr in &self.addresses {
            buf.extend_from_slice(addr.to_string().as_bytes());
            buf.push(0); // разделитель
        }
        buf.extend_from_slice(&self.sequence_number.to_be_bytes());
        buf.extend_from_slice(&self.issued_at.to_be_bytes());
        buf.extend_from_slice(&self.expires_at.to_be_bytes());
        buf
    }

    /// Полная валидация записи.
    pub fn validate(&self) -> Result<(), RecordError> {
        // 1. Восстановить VerifyingKey из байтов
        let pubkey_bytes: [u8; 32] = self.identity_pubkey;
        let pubkey = VerifyingKey::from_bytes(&pubkey_bytes)
            .map_err(|_| RecordError::InvalidKey)?;

        // 2. Проверить что node_id соответствует pubkey
        if !verify_node_id(&self.node_id, &pubkey) {
            return Err(RecordError::NodeIdMismatch);
        }

        // 3. Проверить подпись
        let sig_bytes: [u8; 64] = self.signature.as_slice().try_into()
            .map_err(|_| RecordError::InvalidSignature)?;
        let sig = Signature::from_bytes(&sig_bytes);
        let payload = self.canonical_bytes();
        if !verify_signature(&pubkey, &payload, &sig) {
            return Err(RecordError::InvalidSignature);
        }

        // 4. Проверить TTL
        let now = now_unix();
        if now > self.expires_at {
            return Err(RecordError::Expired);
        }

        // 5. Проверить разумность issued_at (не из далёкого будущего)
        if self.issued_at > now + 60 {
            return Err(RecordError::FutureTimestamp);
        }

        Ok(())
    }

    /// Выбрать «более новую» запись из двух по sequence_number.
    /// Возвращает Err если пришедшая запись является попыткой отката.
    pub fn pick_newer<'a>(
        existing: &'a NodeRecord,
        incoming: &'a NodeRecord,
    ) -> Result<&'a NodeRecord, RecordError> {
        if incoming.sequence_number < existing.sequence_number {
            Err(RecordError::SequenceRollback)
        } else if incoming.sequence_number > existing.sequence_number {
            Ok(incoming)
        } else {
            Ok(existing) // одинаковые — оставляем уже сохранённую
        }
    }
}

#[derive(Debug, Error)]
pub enum RecordError {
    #[error("invalid Ed25519 public key")]
    InvalidKey,
    #[error("node_id does not match public key")]
    NodeIdMismatch,
    #[error("invalid signature")]
    InvalidSignature,
    #[error("record has expired")]
    Expired,
    #[error("issued_at is in the future")]
    FutureTimestamp,
    #[error("incoming sequence_number is lower than stored (rollback attempt)")]
    SequenceRollback,
}
