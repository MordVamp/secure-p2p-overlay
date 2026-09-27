//! types.rs — Общие типы проекта.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::net::SocketAddr;

// ── NodeId ────────────────────────────────────────────────────────────────────

/// 256-битный идентификатор узла (SHA-256 от pubkey).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct NodeId(pub [u8; 32]);

impl NodeId {
    /// XOR-расстояние — беззнаковое 256-битное сравнение (§17).
    pub fn xor_distance(&self, other: &NodeId) -> [u8; 32] {
        let mut d = [0u8; 32];
        for i in 0..32 { d[i] = self.0[i] ^ other.0[i]; }
        d
    }

    /// Индекс k-bucket: позиция старшего установленного бита XOR.
    /// Возвращает 0 если расстояние = 0 (собственный NodeID).
    pub fn bucket_index(&self, other: &NodeId) -> usize {
        let xor = self.xor_distance(other);
        for (byte_idx, &b) in xor.iter().enumerate() {
            if b != 0 {
                return byte_idx * 8 + (7 - b.leading_zeros() as usize);
            }
        }
        0 // расстояние = 0, не должно использоваться для self
    }

    pub fn as_bytes(&self) -> &[u8; 32] { &self.0 }
    pub fn to_hex(&self) -> String { hex::encode(self.0) }
    pub fn short(&self) -> String { hex::encode(&self.0[..4]) }
}

impl fmt::Display for NodeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_hex())
    }
}

impl fmt::Debug for NodeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "NodeId({}…{})", hex::encode(&self.0[..4]), hex::encode(&self.0[28..]))
    }
}

impl TryFrom<&[u8]> for NodeId {
    type Error = anyhow::Error;
    fn try_from(b: &[u8]) -> anyhow::Result<Self> {
        Ok(Self(b.try_into().map_err(|_| anyhow::anyhow!("NodeId must be 32 bytes"))?))
    }
}

// ── Contact ───────────────────────────────────────────────────────────────────

/// Контакт другого узла (§15–16 ТЗ).
///
/// `last_seen_ms` и `last_verified_ms` — локальные метаданные,
/// не принимаются от удалённого узла как достоверные.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Contact {
    pub node_id:            NodeId,
    /// Алгоритм ключа: "ed25519" (§15)
    pub identity_algorithm: String,
    /// Публичный ключ (32 байта Ed25519) (§15)
    pub identity_public_key: Vec<u8>,
    pub host:               String,
    pub port:               u16,
    /// Время последнего наблюдения (unix ms) (§15)
    pub last_seen_ms:       u64,
    /// Время последней успешной верификации PONG (unix ms) (§15)
    pub last_verified_ms:   u64,
}

impl Contact {
    pub fn new(node_id: NodeId, pubkey: Vec<u8>, addr: SocketAddr) -> Self {
        let now = now_ms();
        Self {
            node_id,
            identity_algorithm: "ed25519".to_string(),
            identity_public_key: pubkey,
            host: addr.ip().to_string(),
            port: addr.port(),
            last_seen_ms:     now,
            last_verified_ms: 0,
        }
    }

    pub fn socket_addr(&self) -> anyhow::Result<SocketAddr> {
        Ok(format!("{}:{}", self.host, self.port).parse()?)
    }

    pub fn mark_seen(&mut self) {
        self.last_seen_ms = now_ms();
    }

    pub fn mark_verified(&mut self) {
        let now = now_ms();
        self.last_seen_ms = now;
        self.last_verified_ms = now;
    }
}

impl fmt::Display for Contact {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}@{}:{}", self.node_id.short(), self.host, self.port)
    }
}

// ── Утилиты времени ───────────────────────────────────────────────────────────

/// Unix timestamp в миллисекундах.
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// Unix timestamp в секундах (для обратной совместимости).
pub fn now_unix() -> u64 { now_ms() / 1000 }
