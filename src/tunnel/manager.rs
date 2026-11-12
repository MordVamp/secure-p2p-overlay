//! tunnel/manager.rs — пул туннелей: создание, отправка, переключение на резервный, мониторинг.
//!
//! К каждому получателю держится до `tunnel.pool_size` туннелей (упрощённый уровень — 1, продвинутый — 3):
//! основной строится при первой отправке, остальные достраиваются в фоне, по возможности на
//! непересекающихся ретрансляторах. Отправка идёт по первому пригодному туннелю; если он не подтвердил
//! кадр (TUNNEL_ACK), туннель помечается DEAD и тот же кадр сразу уходит по следующему — переключение на
//! резервный. Если не осталось ни одного, ошибку получает `Messenger`, который перестраивает маршрут
//! через свежий lookup (ADR-21). Выбор ретрансляторов и профили кандидатов — `profile.rs`.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use tokio::sync::{Mutex, RwLock};
use tokio::time::{interval, Duration};
use tracing::{debug, info, warn};

use crate::config::NodeConfig;
use crate::dht::routing_table::RoutingTable;
use crate::tunnel::builder::TunnelBuilder;
use crate::tunnel::forwarder::TunnelForwarder;
use crate::tunnel::profile::{blamed_addr, RelayProfiles};
use crate::tunnel::session::TunnelSession;
use crate::tunnel::state::TunnelState;
use crate::types::{now_ms, Contact, NodeId};

/// Итог отправки через пул.
#[derive(Debug, Clone, Default)]
pub struct SendOutcome {
    /// Основной туннель отказал, кадр доставлен по резервному.
    pub failover:  bool,
    /// От начала отправки до обнаружения отказа (нет TUNNEL_ACK / ERROR), мс.
    pub detect_ms: Option<u64>,
    /// От обнаружения отказа до TUNNEL_ACK по резервному туннелю, мс.
    pub switch_ms: Option<u64>,
    /// Сколько туннелей пула пришлось попробовать.
    pub attempts:  u32,
}

pub struct TunnelManager {
    /// Пулы: target NodeId (hex) → туннели, первый пригодный — основной.
    tunnels:   RwLock<HashMap<String, Vec<TunnelSession>>>,
    builder:   TunnelBuilder,
    cfg:       Arc<NodeConfig>,
    /// A4: отправка данных первому hop (используется задачей-отправителем сессии).
    pub forwarder: Arc<TunnelForwarder>,
    pub profiles:  Arc<Mutex<RelayProfiles>>,
    /// Получатели, для которых уже идёт фоновое пополнение пула.
    filling:   Mutex<HashSet<String>>,
    /// Последний известный контакт получателя (из lookup) — для фонового построения.
    hints:     Mutex<HashMap<String, Contact>>,
}

impl TunnelManager {
    pub fn new(own_contact: Contact, routing: Arc<Mutex<RoutingTable>>, cfg: Arc<NodeConfig>) -> Self {
        let builder = TunnelBuilder::new(own_contact, routing, cfg.clone());
        let profiles = builder.profiles.clone();
        Self {
            tunnels: RwLock::new(HashMap::new()), builder, forwarder: Arc::new(TunnelForwarder::new(cfg.clone())),
            profiles, cfg, filling: Mutex::new(HashSet::new()), hints: Mutex::new(HashMap::new()),
        }
    }

    fn pool_size(&self) -> usize { self.cfg.tunnel.pool_size.max(1) }

    async fn usable_count(&self, key: &str) -> usize {
        let tunnels = self.tunnels.read().await;
        let Some(pool) = tunnels.get(key) else { return 0 };
        let mut n = 0;
        for t in pool { if t.state().await.is_usable() && !t.is_expired() { n += 1; } }
        n
    }

    /// Есть ли пригодный туннель к `target` (без построения).
    pub async fn is_usable(&self, target: &NodeId) -> bool { self.usable_count(&target.to_hex()).await > 0 }

    /// Ретрансляторы пригодных туннелей пула — их по возможности не повторяем в новом туннеле.
    async fn pool_relays(&self, key: &str) -> HashSet<NodeId> {
        let tunnels = self.tunnels.read().await;
        let mut out = HashSet::new();
        if let Some(pool) = tunnels.get(key) {
            for t in pool {
                if t.state().await.is_usable() { out.extend(t.hops.iter().map(|h| h.relay.node_id)); }
            }
        }
        out
    }

    /// Построить один туннель и запустить его задачу-отправителя.
    async fn build_one(&self, target: &NodeId, hint: Option<&Contact>, avoid: &HashSet<NodeId>) -> anyhow::Result<(TunnelSession, u64)> {
        let t0 = now_ms();
        let session = self.builder.build_avoiding(target, hint, avoid).await?;
        let build_ms = now_ms().saturating_sub(t0);
        // TUNNEL_ACK первого relay приходит после ACK всей цепочки → это и есть контроль доступности
        let first = &session.hops.first().ok_or_else(|| anyhow::anyhow!("empty route"))?.relay;
        let (first_hop, first_id) = (first.socket_addr()?, first.node_id);
        if let Some(mut rx) = session.take_outbox().await {
            let fwd = self.forwarder.clone();
            let tag = session.id_hex();
            let health = session.health();
            tokio::spawn(async move {
                while let Some(td) = rx.recv().await {
                    let res = if health.is_dead().await {
                        Err(anyhow::anyhow!("tunnel {tag} is DEAD"))
                    } else {
                        fwd.send_data_to(&td.tunnel_id, first_hop, Some(first_id), td.payload).await.map(|_| ())
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
        Ok((session, build_ms))
    }

    /// Получить пригодный туннель к `target` или построить основной.
    /// Ok(None) — использован существующий, Ok(Some(ms)) — построен новый за `ms`.
    pub async fn get_or_build(self: &Arc<Self>, target: &NodeId) -> anyhow::Result<Option<u64>> {
        self.get_or_build_with(target, None).await
    }

    /// То же с явным контактом получателя (из результата lookup). Пул до `pool_size` пополняется в фоне.
    pub async fn get_or_build_with(self: &Arc<Self>, target: &NodeId, hint: Option<&Contact>) -> anyhow::Result<Option<u64>> {
        let key = target.to_hex();
        if let Some(h) = hint { self.hints.lock().await.insert(key.clone(), h.clone()); }
        let built = if self.usable_count(&key).await == 0 {
            info!("Building tunnel to {}", target.short());
            let (s, ms) = self.build_one(target, hint, &HashSet::new()).await?;
            self.tunnels.write().await.entry(key).or_default().insert(0, s);
            Some(ms)
        } else {
            debug!("Reusing tunnel to {}", target.short());
            None
        };
        self.fill_pool(target);
        Ok(built)
    }

    /// Фоновое пополнение пула до `pool_size` пригодных туннелей.
    fn fill_pool(self: &Arc<Self>, target: &NodeId) {
        if self.pool_size() <= 1 { return; }
        let (me, target) = (self.clone(), *target);
        tokio::spawn(async move {
            let key = target.to_hex();
            if !me.filling.lock().await.insert(key.clone()) { return; }
            for _ in 0..me.pool_size() * 2 {
                if me.usable_count(&key).await >= me.pool_size() { break; }
                let avoid = me.pool_relays(&key).await;
                let hint = me.hints.lock().await.get(&key).cloned();
                match me.build_one(&target, hint.as_ref(), &avoid).await {
                    Ok((s, _)) => {
                        debug!("pool {}: + tunnel {}", target.short(), s.id_hex());
                        me.tunnels.write().await.entry(key.clone()).or_default().push(s);
                    }
                    Err(e) => { debug!("pool {}: пополнение не удалось: {e}", target.short()); break; }
                }
            }
            me.filling.lock().await.remove(&key);
        });
    }

    /// Учесть отказ: виноват узел, на котором оборвалась цепочка; если не определить — все hop туннеля.
    async fn blame(&self, hops: &[Contact], err: &str) {
        let mut p = self.profiles.lock().await;
        match blamed_addr(err) {
            Some(addr) => {
                let by_id = addr.strip_prefix("node:").filter(|h| h.len() >= 8);
                let hit = hops.iter().find(|c| match by_id { Some(h) => c.node_id.to_hex().starts_with(h), None => c.host_port() == addr });
                if let Some(c) = hit { p.failure(c); }
                // иначе адрес — получатель или посторонний: ретрансляторы не виноваты
            }
            None => for c in hops { p.failure(c); },
        }
    }

    /// Отправить данные к `target` и дождаться TUNNEL_ACK; при отказе — по следующему туннелю пула.
    /// Медиакадр звонка (шаг 2): по основному пригодному туннелю к `target`, **мимо очереди туннеля**
    /// (там кадры идут по одному с ожиданием сквозного подтверждения) и без подтверждения вообще.
    /// Ok — кадр отдан первому узлу; доставку никто не подтверждает.
    pub async fn send_media(&self, target: &NodeId, data: Vec<u8>) -> anyhow::Result<()> {
        let (tid, first) = {
            let tunnels = self.tunnels.read().await;
            let pool = tunnels.get(&target.to_hex()).ok_or_else(|| anyhow::anyhow!("no tunnel to {}", target.short()))?;
            let mut pick = None;
            for t in pool {
                if t.state().await.is_usable() && !t.is_expired() {
                    if let Some(h) = t.hops.first() { pick = Some((t.id, h.relay.clone())); break; }
                }
            }
            pick.ok_or_else(|| anyhow::anyhow!("нет пригодного туннеля к {}", target.short()))?
        };
        let frame = crate::transport::framing::Frame {
            version: crate::protocol::PROTOCOL_VERSION,
            msg_type: crate::transport::framing::MsgType::TunnelMedia,
            flags: crate::transport::framing::FrameFlags::empty(),
            request_id: crate::rpc::new_request_id(),
            payload: bytes::Bytes::from(crate::protocol::payload::encode(&crate::protocol::payload::TunnelDataPayload {
                tunnel_id: tid.to_vec(), seq: 0, data,
                hops_left: (self.cfg.tunnel.max_hops + 1).min(u8::MAX as usize) as u8,
            })?),
        };
        crate::rpc::pool::send_oneway(crate::rpc::pool::Scope::Tunnel, first.socket_addr()?, Some(&first.node_id), frame,
            self.cfg.transport.connect_timeout_ms, self.cfg.transport.read_timeout_ms).await
    }

    pub async fn send(self: &Arc<Self>, target: &NodeId, payload: Vec<u8>) -> anyhow::Result<SendOutcome> {
        let key = target.to_hex();
        let t0 = now_ms();
        let mut out = SendOutcome::default();
        let mut detected: Option<u64> = None;
        let mut last_err: Option<anyhow::Error> = None;
        let mut tried: HashSet<[u8; 16]> = HashSet::new();
        loop {
            let pick = {
                let tunnels = self.tunnels.read().await;
                let Some(pool) = tunnels.get(&key) else { anyhow::bail!("no tunnel to {}", target.short()) };
                let mut pick = None;
                for t in pool {
                    if !tried.contains(&t.id) && t.state().await.is_usable() && !t.is_expired() {
                        tried.insert(t.id);
                        let hops: Vec<Contact> = t.hops.iter().map(|h| h.relay.clone()).collect();
                        pick = Some((t.send_confirmed(payload.clone()).await, hops));
                        break;
                    }
                }
                pick
            };
            let Some((rx, hops)) = pick else { break };
            out.attempts += 1;
            let t_try = now_ms();
            let res = match rx {
                Ok(rx) => rx.await.unwrap_or_else(|_| Err(anyhow::anyhow!("tunnel sender stopped"))),
                Err(e) => Err(e),
            };
            match res {
                Ok(()) => {
                    let per_hop = now_ms().saturating_sub(t_try) as f64 / hops.len().max(1) as f64;
                    let mut p = self.profiles.lock().await;
                    for c in &hops { p.success(c, per_hop); }
                    drop(p);
                    if let Some(d) = detected {
                        out.failover = true;
                        out.switch_ms = Some(now_ms().saturating_sub(d));
                        info!("доставка к {} переключена на резервный туннель", target.short());
                        self.fill_pool(target);
                    }
                    return Ok(out);
                }
                Err(e) => {
                    self.blame(&hops, &e.to_string()).await;
                    if detected.is_none() {
                        let now = now_ms();
                        detected = Some(now);
                        out.detect_ms = Some(now.saturating_sub(t0));
                    }
                    last_err = Some(e);
                }
            }
        }
        Err(last_err.unwrap_or_else(|| anyhow::anyhow!("no usable tunnel to {}", target.short())))
    }

    /// Закрыть все туннели к `target`.
    pub async fn close(&self, target: &NodeId) {
        let key = target.to_hex();
        let mut tunnels = self.tunnels.write().await;
        if let Some(pool) = tunnels.get(&key) {
            for t in pool { t.transition(TunnelState::Closing).await; t.transition(TunnelState::Dead).await; }
        }
        tunnels.remove(&key);
        info!("Tunnels to {} closed", target.short());
    }

    /// Фоновый мониторинг: обновляет состояния, убирает мёртвые и истёкшие туннели.
    pub async fn monitor_loop(self: Arc<Self>) {
        let mut ticker = interval(Duration::from_secs(10));
        loop {
            ticker.tick().await;
            self.check_tunnels().await;
        }
    }

    /// DEAD-туннели удаляются; новый маршрут строится при следующей отправке или пополнении пула.
    async fn check_tunnels(&self) {
        let mut dead: Vec<(String, [u8; 16])> = vec![];
        {
            let tunnels = self.tunnels.read().await;
            for (key, pool) in tunnels.iter() {
                for s in pool {
                    s.refresh_state().await;
                    if s.state().await == TunnelState::Dead { dead.push((key.clone(), s.id)); }
                }
            }
        }
        if dead.is_empty() { return; }
        let mut tunnels = self.tunnels.write().await;
        for (key, id) in dead {
            if let Some(pool) = tunnels.get_mut(&key) {
                pool.retain(|s| s.id != id);
                if pool.is_empty() { tunnels.remove(&key); }
            }
            info!("Tunnel to {} removed (DEAD)", &key[..8]);
        }
    }

    /// Диагностика маршрута до `target`: основной туннель (цепочка relay) и пул. None — туннелей нет.
    pub async fn route_json(&self, target: &NodeId) -> Option<serde_json::Value> {
        let tunnels = self.tunnels.read().await;
        let pool = tunnels.get(&target.to_hex())?;
        let mut primary = None;
        let mut list = vec![];
        for t in pool {
            let state = t.state().await;
            let fresh = t.alive_hops() > 0;
            let hops: Vec<_> = t.hops.iter().map(|h| serde_json::json!({
                "index": h.hop_index, "node_id": h.relay.node_id.to_hex(), "addr": h.relay.host_port(),
                "confirmed": h.confirmed, "alive": h.confirmed && fresh,
            })).collect();
            let j = serde_json::json!({
                "tunnel_id": t.id_hex(), "state": format!("{state:?}"), "uptime_ms": t.uptime_ms(),
                "last_ack_age_ms": t.last_ack_age_ms(), "hops": hops,
            });
            if primary.is_none() && state.is_usable() { primary = Some(j.clone()); }
            list.push(j);
        }
        let mut p = primary.or_else(|| list.first().cloned())?;
        p["pool"] = serde_json::json!(list);
        p["pool_size"] = serde_json::json!(self.pool_size());
        p["relay_selection"] = serde_json::json!(self.cfg.tunnel.relay_selection);
        Some(p)
    }

    /// Экспорт состояния всех туннелей в JSON (для метрик и панели).
    pub async fn export_json(&self) -> serde_json::Value {
        let tunnels = self.tunnels.read().await;
        let mut list = vec![];
        for (key, pool) in tunnels.iter() {
            for (i, t) in pool.iter().enumerate() {
                list.push(serde_json::json!({
                    "tunnel_id": t.id_hex(), "target": &key[..8], "role": if i == 0 { "primary" } else { "backup" },
                    "state": format!("{:?}", t.state().await), "hops": t.hops.len(), "alive_hops": t.alive_hops(),
                    "last_ack_age_ms": t.last_ack_age_ms(), "uptime_ms": t.uptime_ms(),
                }));
            }
        }
        serde_json::json!({ "total_tunnels": list.len(), "tunnels": list, "pool_size": self.pool_size(),
                            "relay_selection": self.cfg.tunnel.relay_selection })
    }

    /// Профили кандидатов в ретрансляторы (по убыванию оценки).
    pub async fn profiles_json(&self) -> serde_json::Value { self.profiles.lock().await.json() }
}
