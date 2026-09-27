//! tunnel/forwarder.rs — Пересылка TUNNEL_DATA через цепочку relay.
//!
//! Каждый узел-relay получает TUNNEL_DATA и:
//!  - если он выходной (last hop) — доставляет данные в локальный inbox
//!  - иначе — пересылает следующему hop-у
//!
//! Данные сегментированы: MAX_SEGMENT_SIZE = 48 KiB (§ плана).
//! Каждый сегмент получает seq-номер, ACK возвращается отправителю.

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{Result, bail};
use bytes::Bytes;
use tokio::sync::{Mutex, mpsc, RwLock};
use tracing::{debug, info, warn};

use crate::config::NodeConfig;
use crate::protocol::payload::{TunnelDataPayload, TunnelAckPayload, encode, decode};
use crate::transport::framing::{Frame, FrameFlags, MsgType};
use crate::transport::connection::FramedStream;
use crate::tunnel::session::TunnelId;
use crate::rpc::dial;
use crate::types::now_ms;

pub const MAX_SEGMENT_SIZE: usize = 49_152; // 48 KiB

/// Входящие сообщения для приложения (из туннеля).
pub type InboxSender   = mpsc::Sender<TunnelMessage>;
pub type InboxReceiver = mpsc::Receiver<TunnelMessage>;

#[derive(Debug, Clone)]
pub struct TunnelMessage {
    pub tunnel_id:   TunnelId,
    pub seq:         u64,
    pub data:        Vec<u8>,
    pub received_ms: u64,
}

/// Менеджер пересылки — держит входящую очередь по tunnel_id.
pub struct TunnelForwarder {
    inboxes: RwLock<HashMap<[u8;16], InboxSender>>,
    cfg:     Arc<NodeConfig>,
}

impl TunnelForwarder {
    pub fn new(cfg: Arc<NodeConfig>) -> Self {
        Self { inboxes: RwLock::new(HashMap::new()), cfg }
    }

    /// Зарегистрировать inbox для tunnel_id (вызывается когда туннель становится ACTIVE).
    pub async fn register(&self, tunnel_id: TunnelId) -> InboxReceiver {
        let (tx, rx) = mpsc::channel(128);
        self.inboxes.write().await.insert(tunnel_id, tx);
        rx
    }

    /// Удалить inbox при закрытии туннеля.
    pub async fn unregister(&self, tunnel_id: &TunnelId) {
        self.inboxes.write().await.remove(tunnel_id);
    }

    /// Обработать входящий TUNNEL_DATA frame.
    /// Если мы выходной узел — кладём в inbox.
    /// Иначе — TODO: forward к следующему hop (в упрощённом уровне мы конечный).
    pub async fn handle_data(&self, frame: &Frame) -> Result<Option<Frame>> {
        let payload: TunnelDataPayload = decode(&frame.payload)?;
        let tid: TunnelId = payload.tunnel_id.as_slice().try_into()
            .map_err(|_| anyhow::anyhow!("bad tunnel_id length"))?;

        debug!("TUNNEL_DATA tunnel={} seq={} len={}",
            hex::encode(&tid[..4]), payload.seq, payload.data.len());

        // Доставляем в inbox
        let inboxes = self.inboxes.read().await;
        if let Some(tx) = inboxes.get(&tid) {
            let _ = tx.send(TunnelMessage {
                tunnel_id: tid,
                seq:       payload.seq,
                data:      payload.data,
                received_ms: now_ms(),
            }).await;
        } else {
            warn!("TUNNEL_DATA: no inbox for tunnel {}", hex::encode(&tid[..4]));
        }

        // Отправляем ACK
        let ack = TunnelAckPayload {
            tunnel_id: payload.tunnel_id,
            seq:       payload.seq,
            rtt_ms:    0, // инициатор посчитает сам
        };
        let resp = Frame::new_response(
            MsgType::TunnelAck,
            frame.request_id,
            Bytes::from(encode(&ack)?),
        );
        Ok(Some(resp))
    }

    /// Отправить данные через уже построенный туннель.
    /// Разбивает на сегменты ≤ MAX_SEGMENT_SIZE.
    pub async fn send_data(
        &self,
        tunnel_id: &TunnelId,
        next_hop_addr: std::net::SocketAddr,
        data: Vec<u8>,
    ) -> Result<Vec<u64>> {
        // Разбить на сегменты
        let segments: Vec<&[u8]> = data.chunks(MAX_SEGMENT_SIZE).collect();
        let total = segments.len();
        let mut rtts = Vec::with_capacity(total);

        for (seq, chunk) in segments.iter().enumerate() {
            let t0 = now_ms();
            let payload = TunnelDataPayload {
                tunnel_id: tunnel_id.to_vec(),
                seq:       seq as u64,
                data:      chunk.to_vec(),
            };
            let request_id = crate::rpc::new_request_id();
            let frame = Frame {
                version:    crate::protocol::PROTOCOL_VERSION,
                msg_type:   MsgType::TunnelData,
                flags:      FrameFlags::empty(),
                request_id,
                payload:    Bytes::from(encode(&payload)?),
            };

            let mut stream = dial(
                next_hop_addr,
                self.cfg.transport.connect_timeout_ms,
                self.cfg.transport.read_timeout_ms,
            ).await?;

            stream.send(&frame).await.map_err(|e| anyhow::anyhow!("{e}"))?;

            // Ждём ACK
            use tokio::time::{timeout, Duration};
            let ack_frame = timeout(
                Duration::from_millis(self.cfg.tunnel.ack_timeout_ms),
                stream.recv(),
            ).await
                .map_err(|_| anyhow::anyhow!("ACK timeout for seq={seq}"))??;

            let ack: TunnelAckPayload = decode(&ack_frame.payload)?;
            let rtt = now_ms() - t0;
            rtts.push(rtt);
            debug!("seg seq={} rtt={}ms ACK received", seq, rtt);
        }

        info!("TUNNEL_DATA sent: {} segments, avg_rtt={}ms",
            total,
            if rtts.is_empty() { 0 } else { rtts.iter().sum::<u64>() / rtts.len() as u64 }
        );
        Ok(rtts)
    }
}
