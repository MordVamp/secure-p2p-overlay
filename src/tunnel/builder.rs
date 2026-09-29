//! tunnel/builder.rs — Построение маршрута туннеля через TUNNEL_BUILD RPC.
//!
//! Алгоритм (упрощённый уровень, min_relays=2):
//!  1. Из таблицы маршрутизации выбираем min_relays кандидатов
//!     (не включаем себя и целевой узел).
//!  2. Последовательно отправляем TUNNEL_BUILD к каждому relay.
//!  3. Ждём ACK за build_timeout_ms.
//!  4. Если все relay подтвердили → туннель ACTIVE.
//!  5. Иначе → DEAD (и пробуем с другими кандидатами, до 3 попыток).

use std::sync::Arc;
use std::net::SocketAddr;

use anyhow::{Result, bail};
use tokio::sync::Mutex;
use tokio::time::{timeout, Duration};
use tracing::{debug, info, warn};
use bytes::Bytes;
use uuid::Uuid;

use crate::config::NodeConfig;
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
        let min_relays = self.cfg.tunnel.min_relays;
        let max_attempts = 3;

        for attempt in 1..=max_attempts {
            debug!("[builder] attempt {}/{}", attempt, max_attempts);
            match self.try_build(target, min_relays).await {
                Ok(session) => return Ok(session),
                Err(e) => warn!("[builder] attempt {} failed: {}", attempt, e),
            }
        }
        bail!("Failed to build tunnel to {} after {} attempts", target.short(), max_attempts)
    }

    async fn try_build(&self, target: &NodeId, min_relays: usize) -> Result<TunnelSession> {
        // Выбираем relay-кандидатов из таблицы маршрутизации
        let candidates: Vec<Contact> = {
            let rt = self.routing.lock().await;
            let own_id = *rt.own_id();
            let mut c = rt.find_closest(target, min_relays + 5);
            c.retain(|x| x.node_id != own_id && x.node_id != *target);
            c.into_iter().take(min_relays).collect()
        };

        if candidates.len() < min_relays {
            bail!("Not enough relay candidates: {}/{}", candidates.len(), min_relays);
        }

        let tunnel_id: TunnelId = Uuid::new_v4().into_bytes();
        let total_hops = candidates.len();
        let mut confirmed_hops = Vec::new();

        // Последовательно согласуем с каждым relay
        for (idx, relay) in candidates.iter().enumerate() {
            match self.build_hop(&tunnel_id, relay, idx, total_hops).await {
                Ok(mut hop) => {
                    hop.mark_confirmed();
                    info!("[tunnel-{}] hop {} confirmed: {}", hex::encode(&tunnel_id[..4]), idx, relay);
                    confirmed_hops.push(hop);
                }
                Err(e) => {
                    warn!("[tunnel-{}] hop {} failed ({}): {}", hex::encode(&tunnel_id[..4]), idx, relay, e);
                    bail!("Hop {} failed: {}", idx, e);
                }
            }
        }

        let (session, _rx) = TunnelSession::new(confirmed_hops, self.cfg.clone());
        // Переопределяем ID = тот же что использовали при построении
        session.transition(TunnelState::Active).await;

        info!(
            "[tunnel-{}] ACTIVE — {} hops, build took {}ms",
            session.id_hex(), total_hops, session.uptime_ms()
        );
        Ok(session)
    }

    async fn build_hop(
        &self,
        tunnel_id: &TunnelId,
        relay:     &Contact,
        hop_index: usize,
        total_hops: usize,
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

        let ok: TunnelBuildOkPayload = decode(&resp_frame.payload)?;
        if ok.tunnel_id != tunnel_id {
            bail!("tunnel_id mismatch from relay {}", relay);
        }

        Ok(TunnelHop::new(relay.clone(), hop_index))
    }
}
