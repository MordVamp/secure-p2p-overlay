//! dht/lookup.rs — Итеративный lookup Kademlia (§21 ТЗ).
//!
//! Алгоритм:
//!  1. Начинаем с α ближайших кандидатов из локальной таблицы.
//!  2. Параллельно отправляем FIND_NODE_REQUEST к α неопрошенным ближайшим.
//!  3. Объединяем ответы, обновляем таблицу маршрутизации.
//!  4. Повторяем итерации пока есть прогресс (ближайший контакт становится лучше).
//!  5. Остановка: нет новых кандидатов, ближе цели.
//!  6. Возвращаем K ближайших найденных контактов.

use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::Result;
use tokio::sync::Mutex;
use tracing::{debug, info};
use uuid::Uuid;

use crate::config::NodeConfig;
use crate::dht::routing_table::RoutingTable;
use crate::protocol::payload::FindNodeResponse;
use crate::rpc::{RpcClient, dial};
use crate::types::{Contact, NodeId, now_ms};

/// Результат lookup — ближайшие K контактов.
#[derive(Debug)]
pub struct LookupResult {
    pub lookup_id:   String,
    pub target:      NodeId,
    pub closest:     Vec<Contact>,
    /// Число отправленных FIND_NODE RPC
    pub rpc_count:   usize,
    /// Число итераций
    pub iterations:  usize,
    /// Длительность в мс
    pub duration_ms: u64,
}

/// Итеративный lookup (§21 ТЗ).
pub struct IterativeLookup {
    own_contact: Contact,
    routing:     Arc<Mutex<RoutingTable>>,
    rpc_client:  Arc<RpcClient>,
    cfg:         Arc<NodeConfig>,
}

impl IterativeLookup {
    pub fn new(
        own_contact: Contact,
        routing:     Arc<Mutex<RoutingTable>>,
        rpc_client:  Arc<RpcClient>,
        cfg:         Arc<NodeConfig>,
    ) -> Self {
        Self { own_contact, routing, rpc_client, cfg }
    }

    /// Выполнить lookup для `target`.
    pub async fn run(&self, target: &NodeId) -> Result<LookupResult> {
        let lookup_id = Uuid::new_v4().to_string()[..8].to_string();
        let start = now_ms();
        let k     = self.cfg.dht.k_bucket_size;
        let alpha = self.cfg.dht.alpha;

        info!("[lookup-{}] start target={}", lookup_id, target.short());

        // Начальные кандидаты из локальной таблицы
        let initial = {
            let rt = self.routing.lock().await;
            rt.find_closest(target, k)
        };

        if initial.is_empty() {
            info!("[lookup-{}] no initial candidates", lookup_id);
            return Ok(LookupResult {
                lookup_id, target: *target, closest: vec![],
                rpc_count: 0, iterations: 0,
                duration_ms: now_ms() - start,
            });
        }

        // Состояние lookup
        let mut candidates: Vec<Contact> = initial;
        let mut queried:    HashSet<String> = HashSet::new(); // node_id hex
        let mut found:      HashMap<String, Contact> = HashMap::new();
        let mut rpc_count   = 0usize;
        let mut iterations  = 0usize;
        let mut best_dist   = [0xFFu8; 32]; // XOR-расстояние до лучшего кандидата

        // Сортируем по XOR
        candidates.sort_by_key(|c| c.node_id.xor_distance(target));
        for c in &candidates { found.insert(c.node_id.to_hex(), c.clone()); }

        loop {
            iterations += 1;

            // Выбрать α неопрошенных ближайших
            let to_query: Vec<Contact> = candidates.iter()
                .filter(|c| !queried.contains(&c.node_id.to_hex()))
                .take(alpha)
                .cloned()
                .collect();

            if to_query.is_empty() {
                debug!("[lookup-{}] no more candidates to query", lookup_id);
                break;
            }

            debug!("[lookup-{}] iter={} querying {} peers", lookup_id, iterations, to_query.len());

            // Параллельные FIND_NODE запросы (α штук)
            let mut handles = vec![];
            for contact in to_query {
                queried.insert(contact.node_id.to_hex());
                let own  = self.own_contact.clone();
                let tgt  = *target;
                let cfg  = self.cfg.clone();
                let rpc  = self.rpc_client.clone();

                handles.push(tokio::spawn(async move {
                    let addr: SocketAddr = contact.socket_addr()?;
                    let mut stream = dial(
                        addr,
                        cfg.transport.connect_timeout_ms,
                        cfg.transport.read_timeout_ms,
                    ).await?;
                    let resp = rpc.find_node(&mut stream, &own, &tgt).await?;
                    anyhow::Ok((contact, resp))
                }));
            }

            let results = futures::future::join_all(handles).await;

            let mut improved = false;
            for res in results {
                match res {
                    Ok(Ok((contact, resp))) => {
                        rpc_count += 1;
                        let dist = contact.node_id.xor_distance(target);

                        // Обновляем таблицу маршрутизации
                        {
                            let mut rt = self.routing.lock().await;
                            rt.update(resp.responder.clone());
                        }

                        // Прогресс: нашли узел ближе текущего лучшего?
                        if dist < best_dist {
                            best_dist = dist;
                            improved = true;
                        }

                        // Добавить новые контакты из ответа
                        for c in resp.contacts {
                            let hex = c.node_id.to_hex();
                            if !found.contains_key(&hex) {
                                found.insert(hex, c.clone());
                                candidates.push(c);
                            }
                        }
                    }
                    Ok(Err(e)) => debug!("[lookup-{}] RPC error: {e}", lookup_id),
                    Err(e)     => debug!("[lookup-{}] task error: {e}", lookup_id),
                }
            }

            // Пересортировать кандидатов по XOR
            candidates.sort_by_key(|c| c.node_id.xor_distance(target));
            candidates.truncate(k * 3); // не раздуваем список

            if !improved {
                debug!("[lookup-{}] no improvement, stopping", lookup_id);
                break;
            }
        }

        // K ближайших из найденных
        let mut closest: Vec<Contact> = found.into_values().collect();
        closest.sort_by_key(|c| c.node_id.xor_distance(target));
        closest.truncate(k);

        let duration_ms = now_ms() - start;
        info!(
            "[lookup-{}] done target={} closest={} rpc={} iter={} dur={}ms",
            lookup_id, target.short(), closest.len(), rpc_count, iterations, duration_ms
        );

        Ok(LookupResult {
            lookup_id,
            target: *target,
            closest,
            rpc_count,
            iterations,
            duration_ms,
        })
    }
}
