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
use crate::tunnel::session::{TunnelId, TunnelSession};
use crate::tunnel::state::TunnelState;
use crate::types::{Contact, NodeId};

/// Менеджер пула туннелей.
pub struct TunnelManager {
    /// Активные туннели: target NodeId → сессия
    tunnels:  RwLock<HashMap<String, TunnelSession>>,
    builder:  TunnelBuilder,
    cfg:      Arc<NodeConfig>,
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
            cfg,
        }
    }

    /// Получить активный туннель к `target` или построить новый.
    pub async fn get_or_build(&self, target: &NodeId) -> anyhow::Result<()> {
        let key = target.to_hex();

        // Проверяем наличие пригодного туннеля
        {
            let tunnels = self.tunnels.read().await;
            if let Some(t) = tunnels.get(&key) {
                let state = t.state().await;
                if state.is_usable() {
                    debug!("Reusing tunnel {} to {}", t.id_hex(), target.short());
                    return Ok(());
                }
            }
        }

        // Строим новый
        info!("Building tunnel to {}", target.short());
        let session = self.builder.build(target).await?;

        let mut tunnels = self.tunnels.write().await;
        tunnels.insert(key, session);
        Ok(())
    }

    /// Отправить данные через туннель к `target`.
    pub async fn send(&self, target: &NodeId, payload: Vec<u8>) -> anyhow::Result<()> {
        let key = target.to_hex();
        let tunnels = self.tunnels.read().await;
        let session = tunnels.get(&key)
            .ok_or_else(|| anyhow::anyhow!("no tunnel to {}", target.short()))?;
        session.send(payload).await
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

    /// Фоновый мониторинг: обновляет состояние, восстанавливает упавшие туннели.
    /// Вызывать через `tokio::spawn`.
    pub async fn monitor_loop(self: Arc<Self>) {
        let check_interval = Duration::from_secs(10);
        let mut ticker = interval(check_interval);
        loop {
            ticker.tick().await;
            self.check_tunnels().await;
        }
    }

    async fn check_tunnels(&self) {
        let mut to_rebuild: Vec<(String, NodeId)> = vec![];

        {
            let tunnels = self.tunnels.read().await;
            for (key, session) in tunnels.iter() {
                session.refresh_state().await;
                let state = session.state().await;
                if state == TunnelState::Dead {
                    // Попробуем восстановить — нам нужен NodeId
                    // Получаем его из первого hop-а (выходной узел)
                    if let Some(last) = session.hops.last() {
                        to_rebuild.push((key.clone(), last.relay.node_id));
                    }
                }
            }
        }

        for (key, target) in to_rebuild {
            warn!("Tunnel to {} is DEAD, rebuilding...", &key[..8]);
            let mut tunnels = self.tunnels.write().await;
            tunnels.remove(&key);
            drop(tunnels);

            match self.builder.build(&target).await {
                Ok(new_session) => {
                    info!("Tunnel to {} rebuilt successfully", &key[..8]);
                    self.tunnels.write().await.insert(key, new_session);
                }
                Err(e) => warn!("Failed to rebuild tunnel to {}: {}", &key[..8], e),
            }
        }
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
