//! tunnel/builder.rs — Построение маршрута туннеля через TUNNEL_BUILD RPC.
//!
//! Алгоритм (упрощённый уровень, min_relays=2):
//!  1. Из таблицы маршрутизации выбираем min_relays кандидатов
//!     (не включаем себя и целевой узел).
//!  2. Последовательно отправляем TUNNEL_BUILD к каждому relay.
//!  3. Ждём ACK за build_timeout_ms.
//!  4. Если все relay подтвердили → туннель ACTIVE.
//!  5. Иначе → DEAD и новая попытка без отказавшего relay (до 3 попыток).

use std::sync::Arc;
use std::net::SocketAddr;

use anyhow::{Result, bail};
use tokio::sync::Mutex;
use tokio::time::{timeout, Duration};
use tracing::{debug, info, warn};
use bytes::Bytes;
use uuid::Uuid;

use crate::config::NodeConfig;
use crate::identity::node_identity::verify_node_id_bytes;
use crate::dht::routing_table::RoutingTable;
use crate::rpc::dial;
use crate::transport::framing::{Frame, FrameFlags, MsgType};
use crate::protocol::{PROTOCOL_VERSION, payload::{encode, decode}};
use crate::tunnel::hop::TunnelHop;
use crate::tunnel::session::{TunnelId, TunnelSession};
use crate::tunnel::state::TunnelState;
use crate::types::{Contact, NodeId, now_ms};

pub use crate::protocol::payload::{TunnelBuildPayload, TunnelBuildOkPayload};

/// Строитель туннеля.
pub struct TunnelBuilder {
    own_contact: Contact,
    routing:     Arc<Mutex<RoutingTable>>,
    cfg:         Arc<NodeConfig>,
}

impl TunnelBuilder {
    pub fn new(own_contact: Contact, routing: Arc<Mutex<RoutingTable>>, cfg: Arc<NodeConfig>) -> Self {
        Self { own_contact, routing, cfg }
    }

    /// Построить туннель к `target` через `min_relays` промежуточных узлов.
    /// Возвращает готовую `TunnelSession` в состоянии ACTIVE.
    pub async fn build(&self, target: &NodeId) -> Result<TunnelSession> {
        self.build_with(target, None).await
    }

    /// То же, но адрес получателя можно передать явно (результат lookup): таблица
    /// маршрутизации с малым k может не удержать его, хотя lookup его нашёл.
    pub async fn build_with(&self, target: &NodeId, target_hint: Option<&Contact>) -> Result<TunnelSession> {
        let min_relays = self.cfg.tunnel.min_relays;
        if min_relays > self.cfg.tunnel.max_hops {
            bail!("min_relays={} превышает max_hops={}", min_relays, self.cfg.tunnel.max_hops);
        }
        let max_attempts = 3;
        // relay, отказавшие в предыдущих попытках, больше не выбираются
        let mut excluded = std::collections::HashSet::new();

        for attempt in 1..=max_attempts {
            debug!("[builder] attempt {}/{}", attempt, max_attempts);
            match self.try_build(target, min_relays, target_hint, &mut excluded).await {
                Ok(session) => return Ok(session),
                Err(e) => warn!("[builder] attempt {} failed: {}", attempt, e),
            }
        }
        bail!("Failed to build tunnel to {} after {} attempts", target.short(), max_attempts)
    }

    async fn try_build(&self, target: &NodeId, min_relays: usize, target_hint: Option<&Contact>,
                       excluded: &mut std::collections::HashSet<NodeId>) -> Result<TunnelSession> {
        let t0 = now_ms();
        // Выбираем relay-кандидатов из таблицы маршрутизации
        let candidates: Vec<Contact> = {
            let rt = self.routing.lock().await;
            let own_id = *rt.own_id();
            let mut c = rt.find_closest(target, min_relays + 5 + excluded.len());
            c.retain(|x| x.node_id != own_id && x.node_id != *target && !excluded.contains(&x.node_id));
            // §12: каждый узел в пути не более одного раза (без петель)
            let mut seen = std::collections::HashSet::new();
            c.retain(|x| seen.insert(x.node_id));
            c.into_iter().take(min_relays).collect()
        };

        if candidates.len() < min_relays {
            bail!("Not enough relay candidates: {}/{}", candidates.len(), min_relays);
        }

        // A5: адрес получателя нужен последнему relay (для финальной пересылки)
        // (адрес из lookup приоритетнее таблицы; пара NodeID/pubkey обязана сходиться)
        let from_hint = target_hint
            .filter(|c| c.node_id == *target
                && crate::identity::node_identity::verify_node_id_bytes(&c.node_id, &c.identity_public_key))
            .cloned();
        let target_contact = match from_hint {
            Some(c) => Some(c),
            None => {
                let rt = self.routing.lock().await;
                rt.find_closest(target, 1).into_iter().find(|c| c.node_id == *target)
            }
        }.ok_or_else(|| anyhow::anyhow!("target {} unknown — сначала lookup", target.short()))?;

        let tunnel_id: TunnelId = Uuid::new_v4().into_bytes();
        let total_hops = candidates.len();
        let mut confirmed_hops = Vec::new();

        // Последовательно согласуем с каждым relay
        for (idx, relay) in candidates.iter().enumerate() {
            let next = candidates.get(idx + 1).cloned().unwrap_or_else(|| target_contact.clone());
            match self.build_hop(&tunnel_id, relay, idx, total_hops, Some(next)).await {
                Ok(mut hop) => {
                    hop.mark_confirmed();
                    info!("[tunnel-{}] hop {} confirmed: {}", hex::encode(&tunnel_id[..4]), idx, relay);
                    confirmed_hops.push(hop);
                }
                Err(e) => {
                    warn!("[tunnel-{}] hop {} failed ({}): {}", hex::encode(&tunnel_id[..4]), idx, relay, e);
                    excluded.insert(relay.node_id);
                    bail!("Hop {} failed: {}", idx, e);
                }
            }
        }

        // B3: ID сессии = tunnel_id, согласованный с relay; B6: rx не выбрасываем.
        let (session, rx) = TunnelSession::with_id(tunnel_id, confirmed_hops, self.cfg.clone());
        session.attach_outbox(rx).await;
        session.transition(TunnelState::Active).await;

        // журнал фактической последовательности узлов (§12)
        let path: Vec<String> = session.hops.iter().map(|h| h.relay.node_id.short()).collect();
        info!(
            "[tunnel-{}] ACTIVE — {} hops [{} → {}], build took {}ms",
            session.id_hex(), total_hops, path.join(" → "), target.short(), now_ms().saturating_sub(t0)
        );
        Ok(session)
    }

    async fn build_hop(
        &self,
        tunnel_id: &TunnelId,
        relay:     &Contact,
        hop_index: usize,
        total_hops: usize,
        next_hop:  Option<Contact>,
    ) -> Result<TunnelHop> {
        let addr: SocketAddr = relay.socket_addr()?;
        let mut stream = dial(
            addr,
            self.cfg.transport.connect_timeout_ms,
            self.cfg.transport.read_timeout_ms,
        ).await?;

        let request_id = crate::rpc::new_request_id();
        let payload = TunnelBuildPayload {
            tunnel_id:    tunnel_id.to_vec(),
            initiator:    self.own_contact.clone(),
            hop_index,
            total_hops,
            ttl_seconds:  self.cfg.tunnel.ttl_seconds,
            timestamp_ms: now_ms(),
            next_hop,
        };

        let frame = Frame {
            version:    PROTOCOL_VERSION,
            msg_type:   MsgType::TunnelBuild,
            flags:      FrameFlags::empty(),
            request_id,
            payload:    Bytes::from(encode(&payload)?),
        };

        
        stream.send(&frame).await.map_err(|e| anyhow::anyhow!("{e}"))?;

        // Ждём TUNNEL_BUILD_OK за build_timeout_ms
        let resp_frame = timeout(
            Duration::from_millis(self.cfg.tunnel.build_timeout_ms),
            stream.recv(),
        ).await
            .map_err(|_| anyhow::anyhow!("TUNNEL_BUILD timeout"))?
            .map_err(|e| anyhow::anyhow!("recv: {e}"))?;

        if resp_frame.msg_type == MsgType::TunnelBuildFail || resp_frame.msg_type == MsgType::Error {
            bail!("relay {} refused tunnel build", relay);
        }

        if resp_frame.msg_type != MsgType::TunnelBuildOk {
            bail!("unexpected reply {:?} from relay {}", resp_frame.msg_type, relay);
        }
        if resp_frame.request_id != request_id {
            bail!("request_id mismatch from relay {}", relay);
        }

        let ok: TunnelBuildOkPayload = decode(&resp_frame.payload)?;
        // B3 (аналог E22): relay обязан доказать, что он тот, кого мы выбрали
        if !verify_node_id_bytes(&ok.relay.node_id, &ok.relay.identity_public_key) {
            bail!("relay {} NodeID/pubkey mismatch", relay);
        }
        if ok.relay.node_id != relay.node_id {
            bail!("relay identity differs from requested {}", relay);
        }
        if ok.tunnel_id != tunnel_id {
            bail!("tunnel_id mismatch from relay {}", relay);
        }

        Ok(TunnelHop::new(relay.clone(), hop_index))
    }
}
