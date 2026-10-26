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
               store: Arc::new(crate::dht::storage::DhtStore::new(1024)), metrics }
    }

    /// Процедура bootstrap (§22.2):
    ///  1. PING каждого bootstrap-адреса
    ///  2. Self-lookup
    ///  3. Журнал размера таблицы
    pub async fn join(&self, bootstrap_addrs: &[SocketAddr]) -> Result<()> {
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
        let rec = DhtRecord::new_signed(identity, key, value.clone(), ttl);
        let sig = rec.signature.clone();
        self.store.store(rec).await;
        let mut stored = 1;
        let lk = self.find_node(&crate::types::NodeId(key.0)).await?;
        for c in lk.closest.iter().take(self.cfg.dht.replication_factor) {
            let Ok(addr) = c.socket_addr() else { continue };
            match self.rpc_client.store_value(
                addr, self.cfg.transport.connect_timeout_ms, self.cfg.transport.read_timeout_ms,
                &self.own_contact, &key.0, value.clone(), ttl, sig.clone(),
            ).await {
                Ok(r) if r.stored => stored += 1,
                Ok(_) => {}
                Err(e) => tracing::debug!("STORE to {addr}: {e}"),
            }
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
            if rec.verify() { return Some(rec); }
            tracing::warn!("FIND_VALUE: запись от {addr} с неверной подписью отброшена");
        }
        None
    }
}
