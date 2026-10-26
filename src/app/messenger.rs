//! app/messenger.rs — Высокоуровневый API отправки/приёма E2E сообщений.
//!
//! Шаги отправки:
//!  1. find_node(target) — получить адрес
//!  2. get_or_build_tunnel(target) — построить туннель
//!  3. sign(message) — Ed25519 подпись
//!  4. serialize → send_data через TunnelForwarder
//!
//! Шаги приёма:
//!  1. TunnelForwarder.inbox → TunnelMessage
//!  2. deserialize AppMessage
//!  3. verify signature (Ed25519 + NodeId соответствует pubkey)
//!  4. доставить в приложение через mpsc

use std::sync::Arc;
use std::collections::VecDeque;

use anyhow::{Result, bail};
use tokio::sync::{Mutex, mpsc};
use tracing::{info, warn};

use crate::app::message::AppMessage;
use crate::config::NodeConfig;
use crate::dht::node::DhtNode;
use crate::identity::node_identity::NodeIdentity;
use crate::metrics::MetricsCollector;
use crate::protocol::payload::{encode, decode};
use crate::tunnel::manager::TunnelManager;
use crate::tunnel::forwarder::TunnelForwarder;
use crate::types::NodeId;

/// Входящие подтверждённые сообщения для UI.
pub type MessageRx = mpsc::Receiver<AppMessage>;

pub struct Messenger {
    identity:   Arc<NodeIdentity>,
    dht:        Arc<DhtNode>,
    tunnels:    Arc<TunnelManager>,
    #[allow(dead_code)] forwarder:  Arc<TunnelForwarder>,
    #[allow(dead_code)] metrics:    Arc<MetricsCollector>,
    #[allow(dead_code)] cfg:        Arc<NodeConfig>,
    /// Буфер входящих сообщений (до 100).
    inbox:      Mutex<VecDeque<AppMessage>>,
    /// Анти-replay: ID уже принятых сообщений (ограниченный набор).
    seen:       Mutex<std::collections::HashSet<Vec<u8>>>,
}

impl Messenger {
    pub fn new(
        identity:  Arc<NodeIdentity>,
        dht:       Arc<DhtNode>,
        tunnels:   Arc<TunnelManager>,
        forwarder: Arc<TunnelForwarder>,
        metrics:   Arc<MetricsCollector>,
        cfg:       Arc<NodeConfig>,
    ) -> Self {
        Self { identity, dht, tunnels, forwarder, metrics, cfg, inbox: Mutex::new(VecDeque::new()), seen: Mutex::new(Default::default()) }
    }

    /// Отправить текстовое сообщение узлу `target`.
    pub async fn send_text(&self, target: NodeId, text: &str) -> Result<()> {
        if text.len() > crate::tunnel::forwarder::MAX_SEGMENT_SIZE {
            bail!("Message too large: {} > {}", text.len(), crate::tunnel::forwarder::MAX_SEGMENT_SIZE);
        }

        // 1. Узнать адрес получателя (A5: последний relay должен знать, куда пересылать)
        if let Err(e) = self.dht.find_node(&target).await {
            warn!("lookup {} failed: {e}", target.short());
        }
        self.tunnels.get_or_build(&target).await?;

        // 2. Сформировать и подписать сообщение
        let mut msg = AppMessage::new_text(self.identity.node_id, target, text);
        msg.sender_pubkey = self.identity.public_key_bytes().to_vec();
        let sig = self.identity.sign(&msg.signing_bytes());
        msg.signature = sig.to_bytes().to_vec();

        // 3. Сериализовать
        let data = encode(&msg)?;

        // 4. Отправить через туннель (в текущей реализации через send())
        self.tunnels.send(&target, data).await?;

        info!("→ msg {} to {} ({} bytes)", msg.message_id_hex(), target.short(), text.len());
        Ok(())
    }

    /// Обработать входящий TunnelMessage → проверить подпись → положить в inbox.
    pub async fn handle_incoming(&self, raw: Vec<u8>) -> Result<()> {
        let msg: AppMessage = decode(&raw)
            .map_err(|e| anyhow::anyhow!("AppMessage decode: {e}"))?;

        // Верификация подписи
        if !self.verify_signature(&msg).await {
            bail!("Invalid signature from {}", msg.from.short());
        }

        info!("← msg {} from {} kind={:?}", msg.message_id_hex(), msg.from.short(), msg.kind);

        // Анти-replay
        {
            let mut seen = self.seen.lock().await;
            if seen.len() >= 4096 { seen.clear(); }
            if !seen.insert(msg.message_id.clone()) {
                bail!("duplicate message {}", msg.message_id_hex());
            }
        }

        let mut inbox = self.inbox.lock().await;
        if inbox.len() >= 100 { inbox.pop_front(); }
        inbox.push_back(msg);
        Ok(())
    }

    /// Прочитать и удалить следующее сообщение из inbox.
    pub async fn recv(&self) -> Option<AppMessage> {
        self.inbox.lock().await.pop_front()
    }

    /// Копия последних сообщений без удаления (для GUI).
    pub async fn recent(&self) -> Vec<AppMessage> {
        self.inbox.lock().await.iter().cloned().collect()
    }

    /// Все необработанные сообщения.
    pub async fn recv_all(&self) -> Vec<AppMessage> {
        let mut inbox = self.inbox.lock().await;
        inbox.drain(..).collect()
    }

    /// A6: проверка не зависит от таблицы маршрутизации — ключ берётся из
    /// сообщения и привязан к `from` через NodeID = hash(pubkey).
    async fn verify_signature(&self, msg: &AppMessage) -> bool {
        use crate::identity::node_identity::{verify_node_id_bytes, verify_signature as verify_sig};
        use ed25519_dalek::{VerifyingKey, Signature};

        if msg.to != self.identity.node_id {
            warn!("verify: message addressed to {}, not us", msg.to.short());
            return false;
        }
        if !verify_node_id_bytes(&msg.from, &msg.sender_pubkey) {
            warn!("verify: NodeID/pubkey mismatch for {}", msg.from.short());
            return false;
        }
        let pk: Result<[u8; 32], _> = msg.sender_pubkey.as_slice().try_into();
        let sg: Result<[u8; 64], _> = msg.signature.as_slice().try_into();
        if let (Ok(pk), Ok(sg)) = (pk, sg) {
            if let Ok(vk) = VerifyingKey::from_bytes(&pk) {
                return verify_sig(&vk, &msg.signing_bytes(), &Signature::from_bytes(&sg));
            }
        }
        false
    }
}

// Нужен trait для std::convert::TryInto без явного импорта
use std::convert::TryInto;
