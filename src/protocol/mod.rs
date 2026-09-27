//! protocol/mod.rs — Константы и payload-структуры прикладного протокола.
//!
//! Сериализация: MessagePack (rmp-serde).
//! Примеры payload — см. docs/protocol_spec.md §9.

pub mod payload;
pub use payload::*;

pub const PROTOCOL_VERSION: u8 = 1;
