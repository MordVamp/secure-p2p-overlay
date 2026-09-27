//! dht/storage/ — DHT key-value хранилище (Фаза 4 нашего плана).
//!
//! Операции: STORE, FIND_VALUE.
//! Упрощённый уровень: in-memory HashMap с TTL.
//! Продвинутый уровень: персистентность + репликация (TODO).

pub mod store;
pub mod record;
pub use record::{DhtRecord, RecordKey};
pub use store::DhtStore;
