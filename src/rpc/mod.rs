//! rpc/mod.rs — RPC-слой: PING/PONG, FIND_NODE (§19, §20 ТЗ).

use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::{bail, Result};
use bytes::Bytes;
use tokio::net::TcpStream;
use tokio::sync::{oneshot, Mutex};
use tokio::time::{timeout, Duration};
use tracing::{debug, info, warn};
use uuid::Uuid;

use crate::config::NodeConfig;
use crate::protocol::payload::{self, *};
use crate::transport::framing::{Frame, FrameFlags, MsgType};
use crate::transport::connection::FramedStream;
use crate::types::{Contact, NodeId, now_ms};
use crate::dht::routing_table::RoutingTable;

pub mod pending;
pub use pending::PendingRpc;

// ── Утилиты ───────────────────────────────────────────────────────────────────

/// Сгенерировать новый request_id через CSPRNG (§7 ТЗ).
pub fn new_request_id() -> [u8; 16] {
    Uuid::new_v4().into_bytes()
}

/// Открыть TCP-соединение с тайм-аутом (§5 CONNECT_TIMEOUT_MS).
pub async fn dial(addr: SocketAddr, connect_timeout_ms: u64, read_timeout_ms: u64)
    -> Result<FramedStream<TcpStream>>
{
    let stream = timeout(
        Duration::from_millis(connect_timeout_ms),
        TcpStream::connect(addr),
    ).await
        .map_err(|_| anyhow::anyhow!("connect timeout to {addr}"))?
        .map_err(|e| anyhow::anyhow!("TCP connect to {addr}: {e}"))?;
    Ok(FramedStream::new(stream, read_timeout_ms))
}

fn make_error_frame(request_id: [u8; 16], code: u16, desc: String) -> Frame {
    let err = ErrorPayload { code, description: desc };
    let bytes = payload::encode(&err).unwrap_or_default();
    Frame {
        version:    crate::protocol::PROTOCOL_VERSION,
        msg_type:   MsgType::Error,
        flags:      FrameFlags::IS_RESPONSE | FrameFlags::IS_ERROR,
        request_id,
        payload:    Bytes::from(bytes),
    }
}

// ── RpcHandler ────────────────────────────────────────────────────────────────

pub struct RpcHandler {
    own_contact: Contact,
    routing:     Arc<Mutex<RoutingTable>>,
    pending:     Arc<PendingRpc>,
    cfg:         Arc<NodeConfig>,
}

impl RpcHandler {
    pub fn new(
        own_contact: Contact,
        routing:     Arc<Mutex<RoutingTable>>,
        pending:     Arc<PendingRpc>,
        cfg:         Arc<NodeConfig>,
    ) -> Self { Self { own_contact, routing, pending, cfg } }

    /// Обработать входящий Frame. Возвращает ответный Frame если нужен.
    pub async fn handle(&self, frame: Frame) -> Option<Frame> {
        match frame.msg_type {
            MsgType::Ping => match self.handle_ping(&frame).await {
                Ok(f)  => Some(f),
                Err(e) => { warn!("ping err: {e}"); Some(make_error_frame(frame.request_id, error_codes::BAD_PAYLOAD, e.to_string())) }
            },
            MsgType::Pong => { self.pending.deliver(frame.request_id, frame).await; None }
            MsgType::FindNodeRequest => match self.handle_find_node(&frame).await {
                Ok(f)  => Some(f),
                Err(e) => { warn!("find_node err: {e}"); Some(make_error_frame(frame.request_id, error_codes::BAD_PAYLOAD, e.to_string())) }
            },
            MsgType::FindNodeResponse => { self.pending.deliver(frame.request_id, frame).await; None }
            MsgType::Error => { self.pending.deliver(frame.request_id, frame.clone()).await; None }
            MsgType::StoreRequest    => { self.pending.deliver(frame.request_id, frame).await; None }
            MsgType::StoreResponse   => { self.pending.deliver(frame.request_id, frame).await; None }
            MsgType::FindValueRequest  => { self.pending.deliver(frame.request_id, frame).await; None }
            MsgType::FindValueResponse => { self.pending.deliver(frame.request_id, frame).await; None }
            _ => { debug!("unhandled type {:?}", frame.msg_type); None }
        }
    }

    async fn handle_ping(&self, frame: &Frame) -> Result<Frame> {
        let ping: PingPayload = payload::decode(&frame.payload)?;
        debug!("← PING from {}", ping.sender);
        self.update_routing(ping.sender).await;
        let pong = PongPayload {
            responder:               self.own_contact.clone(),
            ping_timestamp_ms:       ping.timestamp_ms,
            responder_timestamp_ms:  now_ms(),
        };
        let bytes = payload::encode(&pong)?;
        Ok(Frame::new_response(MsgType::Pong, frame.request_id, Bytes::from(bytes)))
    }

    async fn handle_find_node(&self, frame: &Frame) -> Result<Frame> {
        let req: FindNodeRequest = payload::decode(&frame.payload)?;
        let target = NodeId::try_from(req.target_node_id.0.as_slice())?;
        debug!("← FIND_NODE target={}", target.short());
        self.update_routing(req.sender).await;
        let k = self.cfg.dht.k_bucket_size;
        let mut closest = {
            let rt = self.routing.lock().await;
            let own_id = *rt.own_id();
            let mut c = rt.find_closest(&target, k);
            c.retain(|x| x.node_id != own_id);
            c.sort_by_key(|x| x.node_id.xor_distance(&target));
            c.truncate(k);
            c
        };
        let resp = FindNodeResponse {
            responder:      self.own_contact.clone(),
            target_node_id: NodeIdBytes::from(&target),
            contacts:       closest,
        };
        Ok(Frame::new_response(MsgType::FindNodeResponse, frame.request_id, Bytes::from(payload::encode(&resp)?)))
    }

    async fn update_routing(&self, mut contact: Contact) {
        contact.mark_seen();
        let mut rt = self.routing.lock().await;
        rt.update(contact);
    }
}

// ── RpcClient ─────────────────────────────────────────────────────────────────

pub struct RpcClient {
    pending:         Arc<PendingRpc>,
    ping_timeout_ms: u64,
    rpc_timeout_ms:  u64,
}

impl RpcClient {
    pub fn new(pending: Arc<PendingRpc>, cfg: &NodeConfig) -> Self {
        Self { pending, ping_timeout_ms: cfg.dht.ping_timeout_ms, rpc_timeout_ms: cfg.dht.rpc_timeout_ms }
    }

    pub async fn ping<S>(&self, stream: &mut FramedStream<S>, own: &Contact) -> Result<PongPayload>
    where S: tokio::io::AsyncReadExt + tokio::io::AsyncWriteExt + Unpin + Send
    {
        let request_id = new_request_id();
        let ping = PingPayload { sender: own.clone(), timestamp_ms: now_ms() };
        let frame = Frame {
            version: crate::protocol::PROTOCOL_VERSION, msg_type: MsgType::Ping,
            flags: FrameFlags::empty(), request_id,
            payload: Bytes::from(payload::encode(&ping)?),
        };
        let rx = self.pending.register(request_id).await;
        stream.send(&frame).await.map_err(|e| anyhow::anyhow!("{e}"))?;
        let resp = timeout(Duration::from_millis(self.ping_timeout_ms), rx).await
            .map_err(|_| anyhow::anyhow!("PING timeout"))??;
        if resp.msg_type == MsgType::Error {
            let e: ErrorPayload = payload::decode(&resp.payload)?;
            bail!("PING ERROR {}: {}", e.code, e.description);
        }
        let pong: PongPayload = payload::decode(&resp.payload)?;
        info!("PONG from {} rtt={}ms", pong.responder, now_ms() - ping.timestamp_ms);
        Ok(pong)
    }

    pub async fn find_node<S>(&self, stream: &mut FramedStream<S>, own: &Contact, target: &NodeId)
        -> Result<FindNodeResponse>
    where S: tokio::io::AsyncReadExt + tokio::io::AsyncWriteExt + Unpin + Send
    {
        let request_id = new_request_id();
        let req = FindNodeRequest { sender: own.clone(), target_node_id: NodeIdBytes::from(target) };
        let frame = Frame {
            version: crate::protocol::PROTOCOL_VERSION, msg_type: MsgType::FindNodeRequest,
            flags: FrameFlags::empty(), request_id,
            payload: Bytes::from(payload::encode(&req)?),
        };
        let rx = self.pending.register(request_id).await;
        stream.send(&frame).await.map_err(|e| anyhow::anyhow!("{e}"))?;
        let resp = timeout(Duration::from_millis(self.rpc_timeout_ms), rx).await
            .map_err(|_| anyhow::anyhow!("FIND_NODE timeout"))??;
        if resp.msg_type == MsgType::Error {
            let e: ErrorPayload = payload::decode(&resp.payload)?;
            bail!("FIND_NODE ERROR {}: {}", e.code, e.description);
        }
        let r: FindNodeResponse = payload::decode(&resp.payload)?;
        debug!("FIND_NODE_RESPONSE {} contacts", r.contacts.len());
        Ok(r)
    }
}

// ── STORE / FIND_VALUE RPC-клиент (Фаза 4) ───────────────────────────────────

impl RpcClient {
    /// Отправить STORE к peer.
    pub async fn store_value<S>(
        &self, stream: &mut FramedStream<S>, own: &Contact,
        key: &[u8], value: Vec<u8>, ttl_seconds: u64,
    ) -> Result<StoreResponse>
    where S: tokio::io::AsyncReadExt + tokio::io::AsyncWriteExt + Unpin + Send
    {
        let request_id = new_request_id();
        let req = StoreRequest {
            sender: own.clone(), key: key.to_vec(),
            value, ttl_seconds, signature: None,
        };
        let frame = Frame {
            version: crate::protocol::PROTOCOL_VERSION, msg_type: MsgType::StoreRequest,
            flags: FrameFlags::empty(), request_id,
            payload: Bytes::from(payload::encode(&req)?),
        };
        let rx = self.pending.register(request_id).await;
        stream.send(&frame).await.map_err(|e| anyhow::anyhow!("{e}"))?;
        let resp = timeout(Duration::from_millis(self.rpc_timeout_ms), rx).await
            .map_err(|_| anyhow::anyhow!("STORE timeout"))??;
        if resp.msg_type == MsgType::Error {
            let e: ErrorPayload = payload::decode(&resp.payload)?;
            anyhow::bail!("STORE ERROR {}: {}", e.code, e.description);
        }
        let r: StoreResponse = payload::decode(&resp.payload)?;
        Ok(r)
    }

    /// Отправить FIND_VALUE к peer.
    pub async fn find_value<S>(
        &self, stream: &mut FramedStream<S>, own: &Contact, key: &[u8],
    ) -> Result<FindValueResponse>
    where S: tokio::io::AsyncReadExt + tokio::io::AsyncWriteExt + Unpin + Send
    {
        let request_id = new_request_id();
        let req = FindValueRequest { sender: own.clone(), key: key.to_vec() };
        let frame = Frame {
            version: crate::protocol::PROTOCOL_VERSION, msg_type: MsgType::FindValueRequest,
            flags: FrameFlags::empty(), request_id,
            payload: Bytes::from(payload::encode(&req)?),
        };
        let rx = self.pending.register(request_id).await;
        stream.send(&frame).await.map_err(|e| anyhow::anyhow!("{e}"))?;
        let resp = timeout(Duration::from_millis(self.rpc_timeout_ms), rx).await
            .map_err(|_| anyhow::anyhow!("FIND_VALUE timeout"))??;
        if resp.msg_type == MsgType::Error {
            let e: ErrorPayload = payload::decode(&resp.payload)?;
            anyhow::bail!("FIND_VALUE ERROR {}: {}", e.code, e.description);
        }
        let r: FindValueResponse = payload::decode(&resp.payload)?;
        Ok(r)
    }
}
