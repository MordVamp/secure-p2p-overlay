//! protocol/payload.rs — Структуры payload для всех типов сообщений (§9 ТЗ).
//!
//! Каждая структура реализует MessagePack round-trip через rmp-serde.
//! Порядок полей фиксирован и не должен меняться после первой контрольной точки.

use serde::{Deserialize, Serialize};
use crate::types::{Contact, NodeId};

// ── Вспомогательный тип для NodeId в payload ──────────────────────────────────

/// NodeId в сетевом payload — 32 байта.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeIdBytes(pub Vec<u8>);

impl From<&NodeId> for NodeIdBytes {
    fn from(id: &NodeId) -> Self { NodeIdBytes(id.0.to_vec()) }
}

impl TryFrom<NodeIdBytes> for NodeId {
    type Error = anyhow::Error;
    fn try_from(b: NodeIdBytes) -> anyhow::Result<Self> {
        NodeId::try_from(b.0.as_slice())
    }
}

// ── Payload-структуры для PING/PONG ──────────────────────────────────────────

/// `PING` payload (§9.1 ТЗ):
/// ```json
/// { "sender": Contact, "timestamp_ms": uint64 }
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PingPayload {
    pub sender:       Contact,
    pub timestamp_ms: u64,
}

/// `PONG` payload (§9.1 ТЗ):
/// ```json
/// { "responder": Contact, "ping_timestamp_ms": uint64, "responder_timestamp_ms": uint64 }
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PongPayload {
    pub responder:               Contact,
    pub ping_timestamp_ms:       u64,
    pub responder_timestamp_ms:  u64,
}

// ── Payload-структуры для FIND_NODE ─────────────────────────────────────────

/// `FIND_NODE_REQUEST` payload (§9.1 ТЗ):
/// ```json
/// { "sender": Contact, "target_node_id": bytes[32] }
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FindNodeRequest {
    pub sender:         Contact,
    pub target_node_id: NodeIdBytes,
}

/// `FIND_NODE_RESPONSE` payload (§9.1 ТЗ):
/// ```json
/// { "responder": Contact, "target_node_id": bytes[32], "contacts": Contact[] }
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FindNodeResponse {
    pub responder:      Contact,
    pub target_node_id: NodeIdBytes,
    /// Отсортированы по XOR-расстоянию до target (§20.2 ТЗ).
    pub contacts:       Vec<Contact>,
}

// ── ERROR payload ─────────────────────────────────────────────────────────────

/// `ERROR` payload:
/// ```json
/// { "code": uint16, "description": string }
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorPayload {
    pub code:        u16,
    pub description: String,
}

/// Стандартные коды ошибок протокола.
pub mod error_codes {
    pub const BAD_VERSION:       u16 = 1;
    pub const UNKNOWN_TYPE:      u16 = 2;
    pub const PAYLOAD_TOO_LARGE: u16 = 3;
    pub const BAD_PAYLOAD:       u16 = 4;
    pub const BAD_NODE_ID:       u16 = 5;
    pub const TIMEOUT:           u16 = 6;
    pub const INTERNAL:          u16 = 99;
}

// ── MessagePack кодирование/декодирование ────────────────────────────────────

/// Сериализовать payload в MessagePack байты.
pub fn encode<T: Serialize>(val: &T) -> anyhow::Result<Vec<u8>> {
    rmp_serde::to_vec_named(val).map_err(|e| anyhow::anyhow!("msgpack encode: {e}"))
}

/// Десериализовать MessagePack байты в T.
pub fn decode<'a, T: Deserialize<'a>>(bytes: &'a [u8]) -> anyhow::Result<T> {
    rmp_serde::from_slice(bytes).map_err(|e| anyhow::anyhow!("msgpack decode: {e}"))
}
