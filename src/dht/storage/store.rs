//! dht/storage/store.rs — In-memory DHT-хранилище с TTL.
//!
//! Поддерживает: STORE (вставка/обновление), FIND_VALUE (поиск), TTL-очистка.
//! Thread-safe через tokio::sync::RwLock.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::RwLock;
use tracing::{debug, info};

use super::record::{DhtRecord, RecordKey};
use crate::types::NodeId;

/// Учётный размер записи: значение + служебное (ключ, подпись, ключ публикатора, метаданные).
const RECORD_OVERHEAD: usize = 256;
/// Общий объём хранилища по умолчанию.
pub const DEFAULT_MAX_BYTES: usize = 32 * 1024 * 1024;

fn cost(r: &DhtRecord) -> usize { r.value.len() + RECORD_OVERHEAD }

struct Entry {
    rec:  DhtRecord,
    /// Когда запись последний раз была нужна (сохранена, обновлена, прочитана) — номер по счётчику хранилища.
    used: AtomicU64,
}

/// Хранилище записей DHT с TTL и квотами.
///
/// Квоты (ADR-49): на публикатора — 1/16 объёма (2 МиБ при 32 МиБ) и 1/4 числа записей; общий объём и число.
/// Переполнение **не отказ**, а вытеснение: сначала истёкшие записи, затем давно не нужные записи того, кто
/// превысил свою долю (при общей нехватке — самого «крупного» публикатора). Отказ — только если запись сама больше
/// квоты публикатора. Так один узел не вытесняет чужие записи (он вытесняет свои), а честный узел с десятками
/// почтовых ящиков в маленькой сети теряет самые старые неиспользуемые, а не все новые.
pub struct DhtStore {
    records:     RwLock<HashMap<RecordKey, Entry>>,
    max_records: usize,
    max_bytes:   usize,
    /// Монотонный счётчик обращений (миллисекунд недостаточно: за одну мс бывает много записей).
    tick:        AtomicU64,
}

impl DhtStore {
    pub fn new(max_records: usize) -> Self { Self::with_limits(max_records, DEFAULT_MAX_BYTES) }

    pub fn with_limits(max_records: usize, max_bytes: usize) -> Self {
        Self { records: RwLock::new(HashMap::new()), max_records, max_bytes, tick: AtomicU64::new(1) }
    }

    /// Квота одного публикатора: (байт, записей).
    pub fn publisher_quota(&self) -> (usize, usize) {
        ((self.max_bytes / 16).max(64 * 1024), (self.max_records / 4).max(8))
    }

    /// Самая давно не нужная запись среди `filter` (истёкшие — в первую очередь), кроме `except`.
    fn lru_key(map: &HashMap<RecordKey, Entry>, except: &RecordKey, filter: impl Fn(&DhtRecord) -> bool) -> Option<RecordKey> {
        map.iter().filter(|(k, e)| *k != except && filter(&e.rec))
            .min_by_key(|(_, e)| (!e.rec.is_expired(), e.used.load(Ordering::Relaxed)))
            .map(|(k, _)| k.clone())
    }

    /// Сохранить запись. false — отвергнута (откат версии, запись больше квоты публикатора, вытеснить нечего).
    pub async fn store(&self, record: DhtRecord) -> bool {
        let mut map = self.records.write().await;
        // §10.3: откат версии отвергается; равный sequence допустим только как refresh той же записи
        if let Some(old) = map.get(&record.key).map(|e| &e.rec) {
            if !old.is_expired() && old.publisher == record.publisher {
                if record.sequence < old.sequence
                    || (record.sequence == old.sequence && record.value != old.value) {
                    debug!("DhtStore: rollback rejected key={} seq {} < {}",
                           record.key.to_hex(), record.sequence, old.sequence);
                    return false;
                }
            }
        }
        let c = cost(&record);
        let key = record.key.clone();
        let p: NodeId = record.publisher;
        let (pq_bytes, pq_count) = self.publisher_quota();
        if c > pq_bytes {
            debug!("DhtStore: запись {} байт больше квоты публикатора {pq_bytes}", c);
            return false;
        }
        let mut evicted = 0usize;
        // доля публикатора: превышение — вытесняются его же давно не нужные записи
        loop {
            let (b, n) = map.iter().filter(|(k, e)| **k != key && e.rec.publisher == p)
                .fold((0, 0), |(b, n), (_, e)| (b + cost(&e.rec), n + 1));
            if b + c <= pq_bytes && n < pq_count { break; }
            match Self::lru_key(&map, &key, |r| r.publisher == p) {
                Some(k) => { map.remove(&k); evicted += 1; }
                None => return false,
            }
        }
        // общий объём: истёкшие, затем давно не нужные записи самого «крупного» публикатора
        loop {
            let (b, n) = map.iter().filter(|(k, _)| **k != key).fold((0, 0), |(b, n), (_, e)| (b + cost(&e.rec), n + 1));
            if b + c <= self.max_bytes && n < self.max_records { break; }
            let victim = Self::lru_key(&map, &key, |r| r.is_expired()).or_else(|| {
                let mut by_pub: HashMap<NodeId, usize> = HashMap::new();
                for (k, e) in map.iter() { if *k != key { *by_pub.entry(e.rec.publisher).or_default() += cost(&e.rec); } }
                let top = by_pub.into_iter().max_by_key(|(_, b)| *b).map(|(id, _)| id)?;
                Self::lru_key(&map, &key, |r| r.publisher == top)
            });
            match victim {
                Some(k) => { map.remove(&k); evicted += 1; }
                None => return false,
            }
        }
        if evicted > 0 { debug!("DhtStore: вытеснено {evicted} записей ради {}", key.to_hex()); }
        debug!("DhtStore: stored key={} ttl={}s", key.to_hex(), record.ttl_seconds);
        map.insert(key, Entry { rec: record, used: AtomicU64::new(self.tick.fetch_add(1, Ordering::Relaxed)) });
        true
    }

    /// Занято: (байт, записей) — для статуса узла.
    pub async fn usage(&self) -> (usize, usize) {
        let map = self.records.read().await;
        (map.values().map(|e| cost(&e.rec)).sum(), map.len())
    }

    /// Найти запись по ключу. Возвращает None если нет или TTL истёк.
    pub async fn find(&self, key: &RecordKey) -> Option<DhtRecord> {
        let map = self.records.read().await;
        let e = map.get(key)?;
        let r = &e.rec;
        if r.is_expired() {
            debug!("DhtStore: key={} expired", key.to_hex());
            None
        } else {
            e.used.store(self.tick.fetch_add(1, Ordering::Relaxed), Ordering::Relaxed);   // нужна — дольше не вытесняется
            Some(r.clone())
        }
    }

    /// Удалить истёкшие записи (вызывать периодически).
    pub async fn evict_expired(&self) -> usize {
        let mut map = self.records.write().await;
        let before = map.len();
        map.retain(|_, v| !v.rec.is_expired());
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
            .filter(|e| !e.rec.is_expired())
            .map(|e| e.rec.clone())
            .collect()
    }

    /// Экспорт в JSON (§25.3 ТЗ — экспорт данных).
    pub async fn export_json(&self) -> serde_json::Value {
        let map = self.records.read().await;
        let records: Vec<serde_json::Value> = map.values()
            .map(|e| &e.rec)
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
