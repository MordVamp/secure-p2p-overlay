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
use crate::rpc::{RpcClient, PendingRpc, dial};
use crate::types::{Contact, NodeId};

pub struct DhtNode {
    pub own_contact: Contact,
    pub routing:     Arc<Mutex<RoutingTable>>,
    pub pending:     Arc<PendingRpc>,
    pub rpc_client:  Arc<RpcClient>,
    pub cfg:         Arc<NodeConfig>,
}

impl DhtNode {
    pub fn new(own_contact: Contact, cfg: Arc<NodeConfig>) -> Self {
        let routing = Arc::new(Mutex::new(
            RoutingTable::new(own_contact.node_id, cfg.dht.k_bucket_size)
        ));
        let pending    = Arc::new(PendingRpc::new());
        let rpc_client = Arc::new(RpcClient::new(pending.clone(), &cfg));

        Self { own_contact, routing, pending, rpc_client, cfg }
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
        )
    }

    async fn ping_bootstrap(&self, addr: SocketAddr) -> Result<Contact> {
        let mut stream = dial(
            addr,
            self.cfg.transport.connect_timeout_ms,
            self.cfg.transport.read_timeout_ms,
        ).await.context("connect")?;

        let pong = self.rpc_client
            .ping(&mut stream, &self.own_contact)
            .await
            .context("PING")?;

        Ok(pong.responder)
    }
}
