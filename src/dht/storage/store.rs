//! dht/storage/store.rs — In-memory DHT-хранилище с TTL.
//!
//! Поддерживает: STORE (вставка/обновление), FIND_VALUE (поиск), TTL-очистка.
//! Thread-safe через tokio::sync::RwLock.

use std::collections::HashMap;
use tokio::sync::RwLock;
use tracing::{debug, info};

use super::record::{DhtRecord, RecordKey};

pub struct DhtStore {
    records: RwLock<HashMap<RecordKey, DhtRecord>>,
    max_records: usize,
}

impl DhtStore {
    pub fn new(max_records: usize) -> Self {
        Self {
            records:     RwLock::new(HashMap::new()),
            max_records,
        }
    }

    /// Сохранить запись. Возвращает false если хранилище переполнено.
    pub async fn store(&self, record: DhtRecord) -> bool {
        let mut map = self.records.write().await;
        if map.len() >= self.max_records && !map.contains_key(&record.key) {
            debug!("DhtStore full ({} records), rejecting {}", self.max_records, record.key.to_hex());
            return false;
        }
        debug!("DhtStore: stored key={} ttl={}s", record.key.to_hex(), record.ttl_seconds);
        map.insert(record.key, record);
        true
    }

    /// Найти запись по ключу. Возвращает None если нет или TTL истёк.
    pub async fn find(&self, key: &RecordKey) -> Option<DhtRecord> {
        let map = self.records.read().await;
        let r = map.get(key)?;
        if r.is_expired() {
            debug!("DhtStore: key={} expired", key.to_hex());
            None
        } else {
            Some(r.clone())
        }
    }

    /// Удалить истёкшие записи (вызывать периодически).
    pub async fn evict_expired(&self) -> usize {
        let mut map = self.records.write().await;
        let before = map.len();
        map.retain(|_, v| !v.is_expired());
        let removed = before - map.len();
        if removed > 0 {
            info!("DhtStore: evicted {} expired records", removed);
        }
        removed
    }

    /// Число активных записей.
    pub async fn len(&self) -> usize {
        self.records.read().await.len()
    }

    /// Все активные записи (для репликации).
    pub async fn all_records(&self) -> Vec<DhtRecord> {
        self.records.read().await
            .values()
            .filter(|r| !r.is_expired())
            .cloned()
            .collect()
    }

    /// Экспорт в JSON (§25.3 ТЗ — экспорт данных).
    pub async fn export_json(&self) -> serde_json::Value {
        let map = self.records.read().await;
        let records: Vec<serde_json::Value> = map.values()
            .filter(|r| !r.is_expired())
            .map(|r| serde_json::json!({
                "key":           r.key.to_hex(),
                "publisher":     r.publisher.to_hex(),
                "published_ms":  r.published_ms,
                "ttl_seconds":   r.ttl_seconds,
                "remaining_secs": r.remaining_ttl_secs(),
                "value_len":     r.value.len(),
            }))
            .collect();
        serde_json::json!({ "total": records.len(), "records": records })
    }
}

impl Default for DhtStore {
    fn default() -> Self { Self::new(1024) }
}
