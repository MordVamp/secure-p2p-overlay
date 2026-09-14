//! types.rs — Общие типы, используемые по всему проекту.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::net::SocketAddr;

/// 256-битный идентификатор узла (SHA-256 от canonical-encoded pubkey).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord)]
pub struct NodeId(pub [u8; 32]);

impl NodeId {
    /// XOR-расстояние между двумя NodeId (метрика Kademlia).
    pub fn xor_distance(&self, other: &NodeId) -> NodeId {
        let mut d = [0u8; 32];
        for i in 0..32 {
            d[i] = self.0[i] ^ other.0[i];
        }
        NodeId(d)
    }

    /// Номер bucket (длина общего префикса с `other`).
    /// Результат: 0..=255 (0 — самые далёкие).
    pub fn bucket_index(&self, other: &NodeId) -> usize {
        let dist = self.xor_distance(other);
        // Ведущие нулевые биты в XOR = длина общего префикса
        let mut prefix = 0usize;
        for byte in dist.0.iter() {
            if *byte == 0 {
                prefix += 8;
            } else {
                prefix += byte.leading_zeros() as usize;
                break;
            }
        }
        prefix
    }

    pub fn as_hex(&self) -> String {
        hex::encode(self.0)
    }
}

impl fmt::Debug for NodeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "NodeId({}…)", &hex::encode(&self.0[..4]))
    }
}

impl fmt::Display for NodeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", &hex::encode(&self.0[..8]))
    }
}

/// Контакт в routing table.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Contact {
    pub node_id: NodeId,
    pub addr: SocketAddr,
    /// Последнее время наблюдения (Unix timestamp, секунды).
    pub last_seen: u64,
}

impl Contact {
    pub fn new(node_id: NodeId, addr: SocketAddr) -> Self {
        Self {
            node_id,
            addr,
            last_seen: now_unix(),
        }
    }

    pub fn refresh(&mut self) {
        self.last_seen = now_unix();
    }
}

/// Утилита: текущее время в секундах Unix.
pub fn now_unix() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
