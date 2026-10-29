//! identity/alias.rs — Подписанный псевдоним: `alias → NodeID` (§13.1).
//!
//! Запись хранится в DHT под ключом SHA-256("alias:" || alias), значение —
//! `ALIAS1:<alias>`. Владелец — публикатор записи: DhtRecord подписан его
//! identity-ключом (подпись покрывает ключ, NodeID публикатора, sequence и
//! значение), а NodeID = hash(ключа). Хранители принимают ключ по правилу
//! first-writer-wins, пока запись жива (TTL + republish владельцем).

use sha2::{Digest, Sha256};

use crate::dht::storage::{DhtRecord, RecordKey};
use crate::types::NodeId;

const PREFIX: &str = "ALIAS1:";

/// Допустимый псевдоним: 1–32 символа `[a-z0-9_-]`.
pub fn valid_alias(a: &str) -> bool {
    (1..=32).contains(&a.len())
        && a.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
}

pub fn alias_key(alias: &str) -> RecordKey {
    let mut h = Sha256::new();
    h.update(b"alias:");
    h.update(alias.as_bytes());
    RecordKey(h.finalize().into())
}

pub fn alias_value(alias: &str) -> Vec<u8> { format!("{PREFIX}{alias}").into_bytes() }

/// Проверить запись псевдонима и вернуть NodeID владельца.
pub fn owner_of(alias: &str, rec: &DhtRecord) -> Result<NodeId, &'static str> {
    if rec.key != alias_key(alias) { return Err("ключ записи не соответствует псевдониму"); }
    if rec.value != alias_value(alias) { return Err("значение не является записью псевдонима"); }
    if !rec.verify() { return Err("подпись записи неверна"); }
    Ok(rec.publisher)
}
