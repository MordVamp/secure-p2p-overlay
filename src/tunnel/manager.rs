//! tunnel/manager.rs — Пул туннелей: создание, мониторинг, восстановление.
//!
//! Упрощённый уровень: pool_size=1 (один активный туннель к каждому dest).
//! Продвинутый уровень: pool_size≥3, ротация туннелей, профили кандидатов.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::{Mutex, RwLock};
use tokio::time::{interval, Duration};
use tracing::{debug, info, warn};

use crate::config::NodeConfig;
use crate::dht::routing_table::RoutingTable;
use crate::tunnel::builder::TunnelBuilder;
use crate::tunnel::session::TunnelSession;
use crate::tunnel::state::TunnelState;
use crate::tunnel::forwarder::TunnelForwarder;
use crate::types::{Contact, NodeId};

/// Менеджер пула туннелей.
pub struct TunnelManager {
    /// Активные туннели: target NodeId → сессия
    tunnels:  RwLock<HashMap<String, TunnelSession>>,
    builder:  TunnelBuilder,
    #[allow(dead_code)]
    cfg:      Arc<NodeConfig>,
    /// A4: отправка данных первому hop (используется задачей-отправителем сессии).
    pub forwarder: Arc<TunnelForwarder>,
}

impl TunnelManager {
    pub fn new(
        own_contact: Contact,
        routing:     Arc<Mutex<RoutingTable>>,
        cfg:         Arc<NodeConfig>,
    ) -> Self {
        let builder = TunnelBuilder::new(own_contact, routing, cfg.clone());
        Self {
            tunnels: RwLock::new(HashMap::new()),
            builder,
            forwarder: Arc::new(TunnelForwarder::new(cfg.clone())),
            cfg,
        }
    }

    /// Получить активный туннель к `target` или построить новый.
    /// Ok(None) — использован существующий туннель, Ok(Some(ms)) — построен новый за `ms`.
    pub async fn get_or_build(&self, target: &NodeId) -> anyhow::Result<Option<u64>> {
        self.get_or_build_with(target, None).await
    }

    /// Есть ли пригодный туннель к `target` (без построения).
    pub async fn is_usable(&self, target: &NodeId) -> bool {
        let tunnels = self.tunnels.read().await;
        match tunnels.get(&target.to_hex()) {
            Some(t) => t.state().await.is_usable() && !t.is_expired(),
            None => false,
        }
    }

    /// То же с явным контактом получателя (из результата lookup).
    pub async fn get_or_build_with(&self, target: &NodeId, hint: Option<&crate::types::Contact>) -> anyhow::Result<Option<u64>> {
        let key = target.to_hex();

        // Проверяем наличие пригодного туннеля
        {
            let tunnels = self.tunnels.read().await;
            if let Some(t) = tunnels.get(&key) {
                if t.state().await.is_usable() && !t.is_expired() {
                    debug!("Reusing tunnel {} to {}", t.id_hex(), target.short());
                    return Ok(None);
                }
            }
        }

        // Строим новый
        info!("Building tunnel to {}", target.short());
        let t0 = crate::types::now_ms();
        let session = self.builder.build_with(target, hint).await?;
        let build_ms = crate::types::now_ms().saturating_sub(t0);

        // A4: задача-отправитель — забирает очередь сессии и шлёт данные первому hop.
        // TUNNEL_ACK первого relay приходит после ACK всей цепочки → это и есть контроль доступности.
        let first_hop = session.hops.first()
            .ok_or_else(|| anyhow::anyhow!("empty route"))?
            .relay.socket_addr()?;
        if let Some(mut rx) = session.take_outbox().await {
            let fwd = self.forwarder.clone();
            let tag = session.id_hex();
            let health = session.health();
            tokio::spawn(async move {
                while let Some(td) = rx.recv().await {
                    let res = if health.is_dead().await {
                        Err(anyhow::anyhow!("tunnel {tag} is DEAD"))
                    } else {
                        fwd.send_data(&td.tunnel_id, first_hop, td.payload).await.map(|_| ())
                    };
                    match &res {
                        Ok(()) => health.mark_ack().await,
                        Err(e) => {
                            warn!("[tunnel-{tag}] send failed: {e}");
                            health.mark_dead("нет TUNNEL_ACK").await;
                        }
                    }
                    if let Some(done) = td.done { let _ = done.send(res); }
                }
            });
        }

        let mut tunnels = self.tunnels.write().await;
        tunnels.insert(key, session);
        Ok(Some(build_ms))
    }

    /// Отправить данные через туннель к `target` и дождаться TUNNEL_ACK.
    /// Ошибка означает, что кадр не дошёл; туннель при этом помечен DEAD.
    pub async fn send(&self, target: &NodeId, payload: Vec<u8>) -> anyhow::Result<()> {
        let key = target.to_hex();
        let rx = {
            let tunnels = self.tunnels.read().await;
            let session = tunnels.get(&key)
                .ok_or_else(|| anyhow::anyhow!("no tunnel to {}", target.short()))?;
            session.send_confirmed(payload).await?
        };
        rx.await.map_err(|_| anyhow::anyhow!("tunnel sender stopped"))?
    }

    /// Закрыть туннель к `target`.
    pub async fn close(&self, target: &NodeId) {
        let key = target.to_hex();
        let mut tunnels = self.tunnels.write().await;
        if let Some(t) = tunnels.get(&key) {
            t.transition(TunnelState::Closing).await;
            t.transition(TunnelState::Dead).await;
        }
        tunnels.remove(&key);
        info!("Tunnel to {} closed", target.short());
    }

    /// Фоновый мониторинг: обновляет состояние, убирает мёртвые и истёкшие туннели.
    /// Вызывать через `tokio::spawn`.
    pub async fn monitor_loop(self: Arc<Self>) {
        let check_interval = Duration::from_secs(10);
        let mut ticker = interval(check_interval);
        loop {
            ticker.tick().await;
            self.check_tunnels().await;
        }
    }

    /// DEAD-туннели удаляются; новый маршрут к получателю строится при следующей
    /// отправке (Messenger: свежий lookup → TUNNEL_BUILD без отказавших relay → повтор).
    async fn check_tunnels(&self) {
        let mut dead = vec![];
        {
            let tunnels = self.tunnels.read().await;
            for (key, session) in tunnels.iter() {
                session.refresh_state().await;
                if session.state().await == TunnelState::Dead {
                    dead.push((key.clone(), session.id));
                }
            }
        }
        if dead.is_empty() { return; }
        let mut tunnels = self.tunnels.write().await;
        for (key, id) in dead {
            // туннель мог быть заменён новым, пока мы не держали блокировку
            if tunnels.get(&key).map(|s| s.id == id).unwrap_or(false) {
                tunnels.remove(&key);
                info!("Tunnel to {} removed (DEAD), will be rebuilt on next send", &key[..8]);
            }
        }
    }

    /// Диагностика маршрута до `target`: состояние туннеля и цепочка relay (None — туннеля нет).
    pub async fn route_json(&self, target: &NodeId) -> Option<serde_json::Value> {
        let tunnels = self.tunnels.read().await;
        let t = tunnels.get(&target.to_hex())?;
        let fresh = t.alive_hops() > 0;
        let hops: Vec<_> = t.hops.iter().map(|h| serde_json::json!({
            "index":     h.hop_index,
            "node_id":   h.relay.node_id.to_hex(),
            "addr":      format!("{}:{}", h.relay.host, h.relay.port),
            "confirmed": h.confirmed,
            "alive":     h.confirmed && fresh,
        })).collect();
        Some(serde_json::json!({
            "tunnel_id": t.id_hex(),
            "state":     format!("{:?}", t.state().await),
            "uptime_ms": t.uptime_ms(),
            "last_ack_age_ms": t.last_ack_age_ms(),
            "hops":      hops,
        }))
    }

    /// Экспорт состояния всех туннелей в JSON (для метрик).
    pub async fn export_json(&self) -> serde_json::Value {
        let tunnels = self.tunnels.read().await;
        let list: Vec<serde_json::Value> = futures::future::join_all(
            tunnels.values().map(|t| async move {
                serde_json::json!({
                    "tunnel_id": t.id_hex(),
                    "state": format!("{:?}", t.state().await),
                    "hops": t.hops.len(),
                    "alive_hops": t.alive_hops(),
                    "last_ack_age_ms": t.last_ack_age_ms(),
                    "uptime_ms": t.uptime_ms(),
                })
            })
        ).await;

        serde_json::json!({
            "total_tunnels": list.len(),
            "tunnels": list,
        })
    }
}
