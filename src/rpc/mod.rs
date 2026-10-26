//! rpc/mod.rs — RPC-слой: PING/PONG, FIND_NODE (§19, §20 ТЗ).
//!
//! ## Архитектура исходящих соединений (E24-E26)
//!
//! Каждый исходящий запрос использует паттерн:
//!   1. dial() → FramedStream
//!   2. pending.register(request_id) → oneshot::Receiver
//!   3. send(frame)
//!   4. spawn reader_task: читает ответы из потока и вызывает pending.deliver()
//!   5. await rx с тайм-аутом
//!
//! Это позволяет корректно коррелировать ответ с запросом через request_id,
//! даже если соединение будет использовано повторно в будущем.

use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::{bail, Result};
use bytes::Bytes;
use tokio::net::TcpStream;
use tokio::sync::Mutex;
use tokio::time::{timeout, Duration};
use tracing::{debug, info, warn};
use uuid::Uuid;

use crate::config::NodeConfig;
use crate::identity::node_identity::verify_node_id_bytes;
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

/// E1-6 / E24-E26: Запустить фоновую задачу чтения ответов из исходящего соединения.
///
/// Читает кадры из `stream` и доставляет их в `pending` по `request_id`.
/// Задача завершается при EOF или ошибке чтения.
fn spawn_conn_reader(
    mut stream: FramedStream<TcpStream>,
    pending:    Arc<PendingRpc>,
) {
    use crate::transport::framing::ReadItem;
    tokio::spawn(async move {
        loop {
            match stream.recv_item().await {
                ReadItem::Frame(frame) => {
                    debug!("← outbound reader {:?} req={:x?}",
                        frame.msg_type, &frame.request_id[..4]);
                    // Доставляем ответы ожидающим RPC-вызовам
                    pending.deliver(frame.request_id, frame).await;
                }
                ReadItem::Error(e) => {
                    warn!("outbound conn reader error: {e}");
                    break;
                }
                ReadItem::Eof => {
                    debug!("outbound conn reader: EOF");
                    break;
                }
            }
        }
    });
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
            // Ответы — доставляем ожидающему RpcClient
            MsgType::Pong           => { self.pending.deliver(frame.request_id, frame).await; None }
            MsgType::FindNodeResponse => { self.pending.deliver(frame.request_id, frame).await; None }
            MsgType::StoreResponse  => { self.pending.deliver(frame.request_id, frame).await; None }
            MsgType::FindValueResponse => { self.pending.deliver(frame.request_id, frame).await; None }
            MsgType::TunnelBuildOk  => { self.pending.deliver(frame.request_id, frame).await; None }
            MsgType::Error          => { self.pending.deliver(frame.request_id, frame).await; None }

            MsgType::FindNodeRequest => match self.handle_find_node(&frame).await {
                Ok(f)  => Some(f),
                Err(e) => { warn!("find_node err: {e}"); Some(make_error_frame(frame.request_id, error_codes::BAD_PAYLOAD, e.to_string())) }
            },
            MsgType::StoreRequest => match self.handle_store(&frame).await {
                Ok(f)  => Some(f),
                Err(e) => { warn!("store err: {e}"); Some(make_error_frame(frame.request_id, error_codes::BAD_PAYLOAD, e.to_string())) }
            },
            MsgType::FindValueRequest => match self.handle_find_value(&frame).await {
                Ok(f)  => Some(f),
                Err(e) => { warn!("find_value err: {e}"); Some(make_error_frame(frame.request_id, error_codes::BAD_PAYLOAD, e.to_string())) }
            },
            MsgType::TunnelBuild => match self.handle_tunnel_build(&frame).await {
                Ok(f)  => Some(f),
                Err(e) => { warn!("tunnel_build err: {e}"); Some(make_error_frame(frame.request_id, error_codes::BAD_PAYLOAD, e.to_string())) }
            },
            other => {
                warn!("unknown msg_type {:?} — sending ERROR", other);
                Some(make_error_frame(frame.request_id, error_codes::UNKNOWN_TYPE, format!("{other:?}")))
            }
        }
    }

    async fn handle_ping(&self, frame: &Frame) -> Result<Frame> {
        let ping: PingPayload = payload::decode(&frame.payload)?;
        debug!("← PING from {}", ping.sender);

        // E22: Проверить что NodeID соответствует pubkey (§14 ТЗ)
        if !verify_node_id_bytes(&ping.sender.node_id, &ping.sender.identity_public_key) {
            warn!("E22: PING from {} — NodeID/pubkey mismatch, ignoring", ping.sender.node_id.short());
            bail!("NodeID/pubkey mismatch");
        }

        self.update_routing(ping.sender).await;

        let pong = PongPayload {
            responder:              self.own_contact.clone(),
            ping_timestamp_ms:      ping.timestamp_ms,
            responder_timestamp_ms: now_ms(),
        };
        Ok(Frame::new_response(
            MsgType::Pong, frame.request_id,
            Bytes::from(payload::encode(&pong)?),
        ))
    }

    async fn handle_find_node(&self, frame: &Frame) -> Result<Frame> {
        let req: FindNodeRequest = payload::decode(&frame.payload)?;
        let target: NodeId = req.target_node_id.try_into().map_err(|e| anyhow::anyhow!("{e}"))?;
        debug!("← FIND_NODE target={}", target.short());

        // E22: Проверить NodeID/pubkey отправителя
        if !verify_node_id_bytes(&req.sender.node_id, &req.sender.identity_public_key) {
            warn!("E22: FIND_NODE sender {} — NodeID/pubkey mismatch", req.sender.node_id.short());
            bail!("NodeID/pubkey mismatch");
        }

        self.update_routing(req.sender).await;
        let k = self.cfg.dht.k_bucket_size;
        let closest = {
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

    async fn handle_store(&self, frame: &Frame) -> Result<Frame> {
        use crate::dht::storage::{DhtRecord, RecordKey};
        let req: StoreRequest = payload::decode(&frame.payload)?;

        // E22: Проверить NodeID/pubkey
        if !verify_node_id_bytes(&req.sender.node_id, &req.sender.identity_public_key) {
            bail!("NodeID/pubkey mismatch");
        }

        // Конвертируем key Vec<u8> → [u8; 32]
        let key_arr: [u8; 32] = req.key.as_slice().try_into()
            .map_err(|_| anyhow::anyhow!("STORE key must be 32 bytes"))?;
        let key = RecordKey(key_arr);
        let publisher = req.sender.node_id;
        self.update_routing(req.sender).await;

        let _record = DhtRecord::new(key, req.value, publisher, req.ttl_seconds);
        debug!("← STORE key={} ttl={}s", key.to_hex(), req.ttl_seconds);

        let resp = StoreResponse {
            responder: self.own_contact.clone(),
            key: key.0.to_vec(),
            stored: true,
        };
        Ok(Frame::new_response(MsgType::StoreResponse, frame.request_id, Bytes::from(payload::encode(&resp)?)))
    }

    async fn handle_find_value(&self, frame: &Frame) -> Result<Frame> {
        use crate::dht::storage::RecordKey;
        let req: FindValueRequest = payload::decode(&frame.payload)?;

        // E22: Проверить NodeID/pubkey
        if !verify_node_id_bytes(&req.sender.node_id, &req.sender.identity_public_key) {
            bail!("NodeID/pubkey mismatch");
        }

        let key_arr: [u8; 32] = req.key.as_slice().try_into()
            .map_err(|_| anyhow::anyhow!("FIND_VALUE key must be 32 bytes"))?;
        let key = RecordKey(key_arr);
        debug!("← FIND_VALUE key={}", key.to_hex());
        self.update_routing(req.sender).await;

        let resp = FindValueResponse {
            responder: self.own_contact.clone(),
            key: req.key,
            value: None,
            contacts: vec![],
            ttl_remaining_secs: None,
        };
        Ok(Frame::new_response(MsgType::FindValueResponse, frame.request_id, Bytes::from(payload::encode(&resp)?)))
    }

    async fn update_routing(&self, mut contact: Contact) {
        contact.mark_seen();

        // §18.3: Одинаковый NodeID + другой pubkey → отклоняем
        {
            let rt = self.routing.lock().await;
            let known = rt.find_closest(&contact.node_id, 1);
            if let Some(existing) = known.first() {
                if existing.node_id == contact.node_id
                   && existing.identity_public_key != contact.identity_public_key
                {
                    warn!(
                        "§18.3 identity conflict: NodeID {} pubkey mismatch — rejected",
                        contact.node_id.short()
                    );
                    return;
                }
            }
        }

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

    /// E1-6 / E24-E26: PING с корректным чтением ответа через reader-задачу.
    pub async fn ping(
        &self,
        addr:    SocketAddr,
        connect_timeout_ms: u64,
        read_timeout_ms:    u64,
        own:     &Contact,
    ) -> Result<PongPayload> {
        let request_id = new_request_id();
        let ping = PingPayload { sender: own.clone(), timestamp_ms: now_ms() };
        let frame = Frame {
            version: crate::protocol::PROTOCOL_VERSION, msg_type: MsgType::Ping,
            flags: FrameFlags::empty(), request_id,
            payload: Bytes::from(payload::encode(&ping)?),
        };

        let rx = self.pending.register(request_id).await;

        // Dial → send → spawn reader task
        let mut stream = dial(addr, connect_timeout_ms, read_timeout_ms).await?;
        stream.send(&frame).await.map_err(|e| anyhow::anyhow!("{e}"))?;
        spawn_conn_reader(stream.into_raw_stream(), self.pending.clone());

        // Ждём ответ через channel
        let resp = timeout(Duration::from_millis(self.ping_timeout_ms), rx).await
            .map_err(|_| anyhow::anyhow!("PING timeout"))??;
        if resp.msg_type == MsgType::Error {
            let e: ErrorPayload = payload::decode(&resp.payload)?;
            bail!("PING ERROR {}: {}", e.code, e.description);
        }
        let pong: PongPayload = payload::decode(&resp.payload)?;
        // A2 (аналог E22): ответчик обязан соответствовать своему NodeID
        if !verify_node_id_bytes(&pong.responder.node_id, &pong.responder.identity_public_key) {
            bail!("PONG: NodeID/pubkey mismatch from {addr}");
        }
        info!("PONG from {} rtt={}ms", pong.responder, now_ms() - ping.timestamp_ms);
        Ok(pong)
    }

    /// E24-E26: FIND_NODE с reader-задачей для исходящего соединения.
    pub async fn find_node(
        &self,
        addr:    SocketAddr,
        connect_timeout_ms: u64,
        read_timeout_ms:    u64,
        own:     &Contact,
        target:  &NodeId,
    ) -> Result<FindNodeResponse> {
        let request_id = new_request_id();
        let req = FindNodeRequest { sender: own.clone(), target_node_id: NodeIdBytes::from(target) };
        let frame = Frame {
            version: crate::protocol::PROTOCOL_VERSION, msg_type: MsgType::FindNodeRequest,
            flags: FrameFlags::empty(), request_id,
            payload: Bytes::from(payload::encode(&req)?),
        };

        let rx = self.pending.register(request_id).await;

        let mut stream = dial(addr, connect_timeout_ms, read_timeout_ms).await?;
        stream.send(&frame).await.map_err(|e| anyhow::anyhow!("{e}"))?;
        spawn_conn_reader(stream.into_raw_stream(), self.pending.clone());

        let resp = timeout(Duration::from_millis(self.rpc_timeout_ms), rx).await
            .map_err(|_| anyhow::anyhow!("FIND_NODE timeout"))??;
        if resp.msg_type == MsgType::Error {
            let e: ErrorPayload = payload::decode(&resp.payload)?;
            bail!("FIND_NODE ERROR {}: {}", e.code, e.description);
        }
        let mut r: FindNodeResponse = payload::decode(&resp.payload)?;
        // A2: не принимаем в таблицу контакты с поддельной связкой NodeID/pubkey
        let before = r.contacts.len();
        r.contacts.retain(|c| verify_node_id_bytes(&c.node_id, &c.identity_public_key));
        if r.contacts.len() != before {
            warn!("FIND_NODE: отброшено {} контактов с неверным NodeID/pubkey", before - r.contacts.len());
        }
        debug!("FIND_NODE_RESPONSE {} contacts", r.contacts.len());
        Ok(r)
    }
}

// ── STORE / FIND_VALUE RpcClient (Фаза 4) ────────────────────────────────────

impl RpcClient {
    pub async fn store_value(
        &self,
        addr: SocketAddr,
        connect_timeout_ms: u64,
        read_timeout_ms:    u64,
        own:  &Contact,
        key:  &[u8],
        value: Vec<u8>,
        ttl_seconds: u64,
    ) -> Result<StoreResponse> {
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
        let mut stream = dial(addr, connect_timeout_ms, read_timeout_ms).await?;
        stream.send(&frame).await.map_err(|e| anyhow::anyhow!("{e}"))?;
        spawn_conn_reader(stream.into_raw_stream(), self.pending.clone());

        let resp = timeout(Duration::from_millis(self.rpc_timeout_ms), rx).await
            .map_err(|_| anyhow::anyhow!("STORE timeout"))??;
        if resp.msg_type == MsgType::Error {
            let e: ErrorPayload = payload::decode(&resp.payload)?;
            anyhow::bail!("STORE ERROR {}: {}", e.code, e.description);
        }
        Ok(payload::decode(&resp.payload)?)
    }

    pub async fn find_value(
        &self,
        addr: SocketAddr,
        connect_timeout_ms: u64,
        read_timeout_ms:    u64,
        own:  &Contact,
        key:  &[u8],
    ) -> Result<FindValueResponse> {
        let request_id = new_request_id();
        let req = FindValueRequest { sender: own.clone(), key: key.to_vec() };
        let frame = Frame {
            version: crate::protocol::PROTOCOL_VERSION, msg_type: MsgType::FindValueRequest,
            flags: FrameFlags::empty(), request_id,
            payload: Bytes::from(payload::encode(&req)?),
        };
        let rx = self.pending.register(request_id).await;
        let mut stream = dial(addr, connect_timeout_ms, read_timeout_ms).await?;
        stream.send(&frame).await.map_err(|e| anyhow::anyhow!("{e}"))?;
        spawn_conn_reader(stream.into_raw_stream(), self.pending.clone());

        let resp = timeout(Duration::from_millis(self.rpc_timeout_ms), rx).await
            .map_err(|_| anyhow::anyhow!("FIND_VALUE timeout"))??;
        if resp.msg_type == MsgType::Error {
            let e: ErrorPayload = payload::decode(&resp.payload)?;
            anyhow::bail!("FIND_VALUE ERROR {}: {}", e.code, e.description);
        }
        Ok(payload::decode(&resp.payload)?)
    }
}

// ── TUNNEL_BUILD обработчик (Фаза 6) ─────────────────────────────────────────

impl RpcHandler {
    pub async fn handle_tunnel_build(&self, frame: &crate::transport::framing::Frame)
        -> anyhow::Result<crate::transport::framing::Frame>
    {
        use crate::tunnel::builder::{TunnelBuildPayload, TunnelBuildOkPayload};
        let req: TunnelBuildPayload = payload::decode(&frame.payload)?;
        // B3 (аналог E22): проверить NodeID инициатора
        if !verify_node_id_bytes(&req.initiator.node_id, &req.initiator.identity_public_key) {
            anyhow::bail!("TUNNEL_BUILD: initiator NodeID/pubkey mismatch");
        }
        let tid_hex = hex::encode(&req.tunnel_id[..4.min(req.tunnel_id.len())]);
        tracing::info!(
            "← TUNNEL_BUILD id={} hop={}/{} from={}",
            tid_hex, req.hop_index, req.total_hops, req.initiator
        );
        let ok = TunnelBuildOkPayload {
            tunnel_id: req.tunnel_id,
            relay:     self.own_contact.clone(),
            hop_index: req.hop_index,
        };
        Ok(crate::transport::framing::Frame::new_response(
            crate::transport::framing::MsgType::TunnelBuildOk,
            frame.request_id,
            bytes::Bytes::from(payload::encode(&ok)?),
        ))
    }
}
