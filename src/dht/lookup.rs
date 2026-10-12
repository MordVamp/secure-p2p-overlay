//! dht/lookup.rs — Итеративный lookup Kademlia (§21 ТЗ).
//!
//! Алгоритм (A5: first-K-wins вместо «дождаться всех α»):
//!  1. Начинаем с α ближайших кандидатов из локальной таблицы.
//!  2. Запускаем α параллельных FIND_NODE через FuturesUnordered.
//!  3. Обрабатываем ответы по мере поступления — не ждём самый медленный.
//!  4. Повторяем итерации пока есть прогресс (ближайший контакт улучшается).
//!  5. Остановка: нет новых кандидатов, ближе цели.
//!  6. Возвращаем K ближайших найденных контактов.

use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::Result;
use futures::StreamExt;
use tokio::sync::Mutex;
use tracing::{debug, info};
use uuid::Uuid;

use crate::config::NodeConfig;
use crate::dht::routing_table::RoutingTable;
use crate::metrics::{LookupRecord, MetricsCollector};
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

/// Итеративный lookup (§21 ТЗ) + A5: first-K-wins.
pub struct IterativeLookup {
    own_contact: Contact,
    routing:     Arc<Mutex<RoutingTable>>,
    rpc_client:  Arc<RpcClient>,
    cfg:         Arc<NodeConfig>,
    /// Опционально: MetricsCollector для записи CSV (C4)
    metrics:     Option<Arc<MetricsCollector>>,
}

impl IterativeLookup {
    pub fn new(
        own_contact: Contact,
        routing:     Arc<Mutex<RoutingTable>>,
        rpc_client:  Arc<RpcClient>,
        cfg:         Arc<NodeConfig>,
    ) -> Self {
        Self { own_contact, routing, rpc_client, cfg, metrics: None }
    }

    /// Привязать MetricsCollector для записи lookup stats в CSV (C4).
    pub fn with_metrics(mut self, m: Arc<MetricsCollector>) -> Self {
        self.metrics = Some(m);
        self
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
            let res = LookupResult {
                lookup_id, target: *target, closest: vec![],
                rpc_count: 0, iterations: 0,
                duration_ms: now_ms() - start,
            };
            self.record_metrics(&res);
            return Ok(res);
        }

        // Состояние lookup
        let mut candidates: Vec<Contact>         = initial;
        let mut queried:    HashSet<String>       = HashSet::new(); // node_id hex
        let mut found:      HashMap<String, Contact> = HashMap::new();
        let mut rpc_count   = 0usize;
        let mut iterations  = 0usize;
        let mut best_dist   = [0xFFu8; 32];

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

            // ── A5: FuturesUnordered — обрабатываем ответы по мере прихода ──
            let mut stream = futures::stream::FuturesUnordered::new();

            for contact in to_query {
                queried.insert(contact.node_id.to_hex());
                let own  = self.own_contact.clone();
                let tgt  = *target;
                let cfg  = self.cfg.clone();
                let rpc  = self.rpc_client.clone();

                stream.push(tokio::spawn(async move {
                    let addr: SocketAddr = contact.socket_addr()?;
                    let mut s = dial(
                        addr,
                        cfg.transport.connect_timeout_ms,
                        cfg.transport.read_timeout_ms,
                    ).await?;
                    let resp = rpc.find_node(&mut s, &own, &tgt).await?;
                    anyhow::Ok((contact, resp))
                }));
            }

            let mut improved = false;

            // Читаем ответы как только приходят (не ждём самый медленный)
            while let Some(res) = stream.next().await {
                match res {
                    Ok(Ok((contact, resp))) => {
                        rpc_count += 1;
                        let dist = contact.node_id.xor_distance(target);

                        // Обновляем таблицу маршрутизации
                        {
                            let mut rt = self.routing.lock().await;
                            rt.update(resp.responder.clone());
                        }

                        // Прогресс?
                        if dist < best_dist {
                            best_dist = dist;
                            improved = true;
                        }

                        // Добавить новые контакты
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

            // Пересортировать, не раздувать список
            candidates.sort_by_key(|c| c.node_id.xor_distance(target));
            candidates.truncate(k * 3);

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

        let res = LookupResult {
            lookup_id,
            target: *target,
            closest,
            rpc_count,
            iterations,
            duration_ms,
        };

        self.record_metrics(&res);
        Ok(res)
    }

    /// C4: Записать статистику lookup в CSV (если MetricsCollector подключён).
    fn record_metrics(&self, r: &LookupResult) {
        if let Some(m) = &self.metrics {
            m.record_lookup(&LookupRecord {
                ts_ms:         crate::types::now_ms(),
                lookup_id:     r.lookup_id.clone(),
                initiator:     self.own_contact.node_id.to_hex()[..8].to_string(),
                target:        r.target.to_hex(),
                found:         !r.closest.is_empty(),
                rpc_count:     r.rpc_count,
                iterations:    r.iterations,
                duration_ms:   r.duration_ms,
                closest_count: r.closest.len(),
            });
        }
    }
}
