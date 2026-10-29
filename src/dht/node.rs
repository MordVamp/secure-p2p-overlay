//! dht/node.rs — Фасад DHT: join, find_node, export (§22 ТЗ).
//!
//! `DhtNode::join()` выполняет процедуру bootstrap (§22.2):
//!  1. PING каждого bootstrap-контакта (подтверждение + добавление в таблицу)
//!  2. Self-lookup: FIND_NODE(own_id)
//!  3. Журнал итогового состояния таблицы маршрутизации

use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::{Result, Context};
use tokio::sync::Mutex;
use tracing::{info, warn};

use crate::config::NodeConfig;
use crate::dht::lookup::IterativeLookup;
use crate::dht::routing_table::RoutingTable;
use crate::rpc::{RpcClient, PendingRpc};
use crate::types::{Contact, NodeId};

pub struct DhtNode {
    pub own_contact: Contact,
    pub routing:     Arc<Mutex<RoutingTable>>,
    pub pending:     Arc<PendingRpc>,
    pub rpc_client:  Arc<RpcClient>,
    pub cfg:         Arc<NodeConfig>,
    /// DHT key-value хранилище (STORE/FIND_VALUE) — Фаза 4
    pub store:       Arc<crate::dht::storage::DhtStore>,
    /// Метрики (C4)
    pub metrics:     Arc<crate::metrics::MetricsCollector>,
    /// Записи, опубликованные этим узлом (для переопубликования, §10.1).
    pub published:   Mutex<std::collections::HashMap<RecordKey, (Vec<u8>, u64)>>,
    /// Создание узла (unix ms) и завершение bootstrap (0 — ещё идёт) — метрика холодного старта.
    pub created_ms:  u64,
    pub joined_ms:   std::sync::atomic::AtomicU64,
}

impl DhtNode {
    pub fn new(own_contact: Contact, cfg: Arc<NodeConfig>) -> Self {
        let routing = Arc::new(Mutex::new(
            RoutingTable::new(own_contact.node_id, cfg.dht.k_bucket_size)
        ));
        let pending    = Arc::new(PendingRpc::new());
        let rpc_client = Arc::new(RpcClient::new(pending.clone(), &cfg));

        let metrics = Arc::new(
            crate::metrics::MetricsCollector::new(&cfg.node.metrics_dir, &own_contact.node_id)
        );

        Self { own_contact, routing, pending, rpc_client, cfg,
               store: Arc::new(crate::dht::storage::DhtStore::new(1024)), metrics,
               published: Mutex::new(std::collections::HashMap::new()),
               created_ms: crate::types::now_ms(), joined_ms: Default::default() }
    }

    /// Время bootstrap: от создания узла до завершения join (PING bootstrap + self-lookup).
    pub fn bootstrap_json(&self) -> serde_json::Value {
        let j = self.joined_ms.load(std::sync::atomic::Ordering::Relaxed);
        serde_json::json!({
            "started_ms": self.created_ms,
            "joined_ms":  if j == 0 { None } else { Some(j) },
            "join_ms":    if j == 0 { None } else { Some(j.saturating_sub(self.created_ms)) },
        })
    }

    /// Процедура bootstrap (§22.2):
    ///  1. PING каждого bootstrap-адреса
    ///  2. Self-lookup
    ///  3. Журнал размера таблицы
    pub async fn join(&self, bootstrap_addrs: &[SocketAddr]) -> Result<()> {
        let r = self.join_inner(bootstrap_addrs).await;
        self.joined_ms.store(crate::types::now_ms(), std::sync::atomic::Ordering::Relaxed);
        r
    }

    async fn join_inner(&self, bootstrap_addrs: &[SocketAddr]) -> Result<()> {
        if bootstrap_addrs.is_empty() {
            info!("No bootstrap peers — running as seed node");
            return Ok(());
        }

        info!("Bootstrap: connecting to {} peers", bootstrap_addrs.len());

        // Шаг 1: PING каждому bootstrap-узлу
        for &addr in bootstrap_addrs {
            match self.ping_bootstrap(addr).await {
                Ok(contact) => {
                    info!("Bootstrap peer {} ({}) reachable, NodeID={}",
                          addr, contact.node_id.short(), contact.node_id);
                    let mut rt = self.routing.lock().await;
                    rt.update(contact);
                }
                Err(e) => warn!("Bootstrap peer {addr} unreachable: {e}"),
            }
        }

        // Шаг 2: Self-lookup
        info!("Starting self-lookup NodeID={}", self.own_contact.node_id.short());
        let lookup = self.make_lookup();
        match lookup.run(&self.own_contact.node_id).await {
            Ok(res) => {
                info!(
                    "Self-lookup done: {} closest contacts, {} RPC, {} iter, {}ms",
                    res.closest.len(), res.rpc_count, res.iterations, res.duration_ms
                );
            }
            Err(e) => warn!("Self-lookup error: {e}"),
        }

        // Шаг 3: Журнал итогового состояния
        let rt = self.routing.lock().await;
        info!(
            "Routing table: {} contacts in {} non-empty buckets",
            rt.total_contacts(),
            rt.non_empty_buckets()
        );

        Ok(())
    }

    /// Выполнить lookup для произвольного target.
    pub async fn find_node(&self, target: &NodeId) -> Result<crate::dht::lookup::LookupResult> {
        self.make_lookup().run(target).await
    }

    /// Экспортировать таблицу маршрутизации в JSON (§25.3 ТЗ).
    pub async fn export_routing_table(&self) -> serde_json::Value {
        let rt = self.routing.lock().await;
        let buckets: Vec<serde_json::Value> = rt.snapshot().iter().map(|(idx, contacts)| {
            serde_json::json!({
                "bucket_index": idx,
                "contacts": contacts.iter().map(|c| serde_json::json!({
                    "node_id": c.node_id.to_hex(),
                    "host":    c.host,
                    "port":    c.port,
                    "last_seen_ms": c.last_seen_ms,
                })).collect::<Vec<_>>()
            })
        }).collect();

        serde_json::json!({
            "own_node_id": rt.own_id().to_hex(),
            "k": rt.k(),
            "total_contacts": rt.total_contacts(),
            "non_empty_buckets": rt.non_empty_buckets(),
            "buckets": buckets,
        })
    }

    fn make_lookup(&self) -> IterativeLookup {
        IterativeLookup::new(
            self.own_contact.clone(),
            self.routing.clone(),
            self.rpc_client.clone(),
            self.cfg.clone(),
        ).with_metrics(self.metrics.clone())
    }

    async fn ping_bootstrap(&self, addr: SocketAddr) -> Result<Contact> {
        // E1-6: RpcClient::ping теперь сам открывает соединение и запускает reader-задачу
        let pong = self.rpc_client
            .ping(
                addr,
                self.cfg.transport.connect_timeout_ms,
                self.cfg.transport.read_timeout_ms,
                &self.own_contact,
            )
            .await
            .context("PING")?;
        Ok(pong.responder)
    }
}

// ── Store/Value поддержка (Фаза 4) ──────────────────────────────────────────

use crate::dht::storage::{DhtRecord, RecordKey};

impl DhtNode {
    /// Сохранить значение в локальном хранилище.
    pub async fn store_local(&self, record: DhtRecord) -> bool {
        self.store.store(record).await
    }

    /// Найти значение локально.
    pub async fn find_value_local(&self, key: &RecordKey) -> Option<DhtRecord> {
        self.store.find(key).await
    }

    /// Экспорт хранилища в JSON.
    pub async fn export_store(&self) -> serde_json::Value {
        self.store.export_json().await
    }
}

// ── A7/A8: подписанные DHT put/get с репликацией ─────────────────────────────
impl DhtNode {
    /// STORE: подписать запись, сохранить локально и на R ближайших узлах.
    /// Возвращает число узлов (включая себя), принявших запись.
    pub async fn put(
        &self,
        identity: &crate::identity::node_identity::NodeIdentity,
        key: RecordKey,
        value: Vec<u8>,
        ttl: u64,
    ) -> Result<usize> {
        let ttl = ttl.min(self.cfg.dht.ttl_seconds);
        if let Some(ex) = self.store.find(&key).await {
            if ex.publisher != identity.node_id {
                anyhow::bail!("ключ принадлежит другому публикатору");
            }
        }
        // sequence строго растёт: не меньше предыдущего+1 и не меньше текущего времени (мс)
        let seq = self.store.find(&key).await.map(|r| r.sequence + 1).unwrap_or(0)
            .max(crate::types::now_ms());
        let rec = DhtRecord::new_signed_seq(identity, key, value.clone(), ttl, seq);
        let sig = rec.signature.clone();
        if !self.store.store(rec).await { anyhow::bail!("локальная запись отклонена (откат версии?)"); }
        self.published.lock().await.insert(key, (value.clone(), ttl));
        let mut stored = 1;
        let lk = self.find_node(&crate::types::NodeId(key.0)).await?;
        for c in lk.closest.iter().take(self.cfg.dht.replication_factor) {
            let Ok(addr) = c.socket_addr() else { continue };
            match self.rpc_client.store_value(
                addr, self.cfg.transport.connect_timeout_ms, self.cfg.transport.read_timeout_ms,
                &self.own_contact, &key.0, value.clone(), ttl, sig.clone(), seq,
            ).await {
                Ok(r) if r.stored => stored += 1,
                Ok(_) => {}
                Err(e) => tracing::debug!("STORE to {addr}: {e}"),
            }
        }
        // §10.1: успех только при кворуме. Локальная копия остаётся (её подхватит republish).
        let quorum = self.cfg.dht.store_quorum.min(self.cfg.dht.replication_factor).max(1);
        if stored < quorum {
            anyhow::bail!("STORE: подтверждено {stored} из {} реплик, нужно ≥{quorum}",
                          self.cfg.dht.replication_factor);
        }
        Ok(stored)
    }

    /// FIND_VALUE: локально, затем у ближайших узлов. Принимается только запись
    /// с валидной подписью публикатора.
    pub async fn get(&self, key: &RecordKey) -> Option<DhtRecord> {
        if let Some(r) = self.store.find(key).await { return Some(r); }
        let lk = self.find_node(&crate::types::NodeId(key.0)).await.ok()?;
        for c in lk.closest.iter().take(self.cfg.dht.replication_factor + 2) {
            let Ok(addr) = c.socket_addr() else { continue };
            let Ok(resp) = self.rpc_client.find_value(
                addr, self.cfg.transport.connect_timeout_ms, self.cfg.transport.read_timeout_ms,
                &self.own_contact, &key.0,
            ).await else { continue };
            let (Some(v), Some(sig)) = (resp.value, resp.signature) else { continue };
            let Ok(publisher) = crate::identity::node_identity::compute_node_id_from_bytes(&resp.publisher_key)
                else { continue };
            let mut rec = DhtRecord::new(*key, v, publisher, resp.ttl_remaining_secs.unwrap_or(1));
            rec.publisher_key = resp.publisher_key;
            rec.signature = Some(sig);
            rec.sequence = resp.sequence;
            if rec.verify() { return Some(rec); }
            tracing::warn!("FIND_VALUE: запись от {addr} с неверной подписью отброшена");
        }
        None
    }
}

// ── §10.1: переопубликование и TTL-очистка ───────────────────────────────────
impl DhtNode {
    /// Переопубликовать все свои записи (новый sequence, свежий TTL). Возвращает число успешных.
    pub async fn republish_all(&self, identity: &crate::identity::node_identity::NodeIdentity) -> usize {
        let items: Vec<_> = self.published.lock().await.iter()
            .map(|(k, (v, t))| (*k, v.clone(), *t)).collect();
        let mut ok = 0;
        for (k, v, t) in items {
            // NodeRecord пересоздаётся (новые issued_at/expires_at), а не повторяется как есть
            if k == RecordKey(crate::identity::record::node_record_key(&identity.node_id)) { continue; }
            match self.put(identity, k, v, t).await {
                Ok(n) => { ok += 1; tracing::debug!("republish {} → {} узлов", k.to_hex(), n); }
                Err(e) => tracing::warn!("republish {} failed: {e}", k.to_hex()),
            }
        }
        ok
    }
}

// ── §10.3: NodeRecord (адресная запись узла) ─────────────────────────────────
use crate::identity::record::{node_record_key, NodeRecord};

impl DhtNode {
    /// Опубликовать/обновить свою подписанную адресную запись под ключом SHA-256("node:"||NodeID).
    pub async fn publish_node_record(
        &self,
        identity: &crate::identity::node_identity::NodeIdentity,
    ) -> Result<usize> {
        let addr = self.own_contact.socket_addr()
            .map_err(|e| anyhow::anyhow!("own address: {e}"))?;
        let ttl = self.cfg.dht.ttl_seconds;
        let rec = NodeRecord::new(
            identity.node_id, identity.public_key_bytes(), vec![addr],
            crate::types::now_ms(), ttl,
            |b| identity.sign(b).to_bytes().to_vec(),
        );
        let key = RecordKey(node_record_key(&identity.node_id));
        let res = self.put(identity, key, rec.to_value()?, ttl).await;
        // свою запись пересоздаёт publish_node_record, а не republish_all
        self.published.lock().await.remove(&key);
        res
    }

    /// Найти адресную запись узла по NodeID и проверить её (подпись, TTL, привязка к ключу).
    pub async fn resolve_node(&self, target: &NodeId) -> Result<NodeRecord> {
        let key = RecordKey(node_record_key(target));
        let dr = self.get(&key).await.ok_or_else(|| anyhow::anyhow!("NodeRecord не найден"))?;
        let nr = NodeRecord::from_value(&dr.value)
            .ok_or_else(|| anyhow::anyhow!("значение не является NodeRecord"))??;
        nr.validate_for_key(&key.0).map_err(|e| anyhow::anyhow!("NodeRecord отвергнут: {e}"))?;
        if nr.node_id != *target { anyhow::bail!("NodeRecord принадлежит другому узлу"); }
        Ok(nr)
    }
}

// ── §13.1: подписанный псевдоним (alias → NodeID) ────────────────────────────
use crate::identity::alias::{alias_key, alias_value, owner_of, valid_alias};

impl DhtNode {
    /// Опубликовать псевдоним этого узла. Занятый другим узлом псевдоним не перезаписывается.
    pub async fn publish_alias(
        &self,
        identity: &crate::identity::node_identity::NodeIdentity,
        alias: &str,
    ) -> Result<usize> {
        if !valid_alias(alias) { anyhow::bail!("псевдоним: 1–32 символа [a-z0-9_-]"); }
        if let Ok(owner) = self.resolve_alias(alias).await {
            if owner != identity.node_id {
                anyhow::bail!("псевдоним «{alias}» уже занят узлом {}", owner.short());
            }
        }
        self.put(identity, alias_key(alias), alias_value(alias), self.cfg.dht.ttl_seconds).await
    }

    /// Разрешить псевдоним в NodeID владельца (подпись записи проверяется).
    pub async fn resolve_alias(&self, alias: &str) -> Result<NodeId> {
        if !valid_alias(alias) { anyhow::bail!("псевдоним: 1–32 символа [a-z0-9_-]"); }
        let rec = self.get(&alias_key(alias)).await
            .ok_or_else(|| anyhow::anyhow!("псевдоним «{alias}» не найден"))?;
        owner_of(alias, &rec).map_err(|e| anyhow::anyhow!("псевдоним «{alias}»: {e}"))
    }
}
