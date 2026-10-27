//! app/messenger.rs — Высокоуровневый API отправки/приёма E2E сообщений.
//!
//! Шаги отправки:
//!  1. find_node(target) — получить адрес
//!  2. get_or_build_tunnel(target) — построить туннель
//!  3. sign(message) — Ed25519 подпись, затем E2E-шифрование (security/e2e.rs)
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
use tokio::sync::{Mutex, mpsc, oneshot};
use tracing::{info, warn};

use crate::app::message::{AppMessage, MessageKind};
use crate::config::NodeConfig;
use crate::dht::node::DhtNode;
use crate::identity::node_identity::NodeIdentity;
use crate::metrics::MetricsCollector;
use crate::protocol::payload::{encode, decode};
use crate::tunnel::manager::TunnelManager;
use crate::tunnel::forwarder::TunnelForwarder;
use crate::types::{NodeId, now_ms};

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
    /// APP_ACK: message_id → (кому отправили, канал пробуждения отправителя).
    waiting:    Mutex<std::collections::HashMap<Vec<u8>, (NodeId, oneshot::Sender<()>)>>,
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
        Self { identity, dht, tunnels, forwarder, metrics, cfg, inbox: Mutex::new(VecDeque::new()), seen: Mutex::new(Default::default()), waiting: Mutex::new(Default::default()) }
    }

    /// Собрать и подписать текстовое сообщение.
    fn build_text(&self, target: NodeId, text: &str) -> Result<AppMessage> {
        if text.len() > crate::tunnel::forwarder::MAX_SEGMENT_SIZE - 1024 {
            bail!("Message too large: {} > {}", text.len(), crate::tunnel::forwarder::MAX_SEGMENT_SIZE - 1024);
        }
        let mut msg = AppMessage::new_text(self.identity.node_id, target, text);
        self.sign(&mut msg);
        Ok(msg)
    }

    fn sign(&self, msg: &mut AppMessage) {
        msg.sender_pubkey = self.identity.public_key_bytes().to_vec();
        msg.signature = self.identity.sign(&msg.signing_bytes()).to_bytes().to_vec();
    }

    /// Доставить подписанное сообщение: lookup → туннель → E2E-шифрование → отправка.
    /// `pubkey_hint` — ключ получателя, если он уже известен (например, из входящего сообщения).
    async fn deliver(&self, target: NodeId, pubkey_hint: Option<&[u8]>, msg: &AppMessage) -> Result<()> {
        // Узнать адрес получателя (A5: последний relay должен знать, куда пересылать)
        // Контакт получателя берём из результата lookup, а не только из таблицы
        // маршрутизации: при малом k корзина может быть полна и не удержать найденный узел.
        let found = match self.dht.find_node(&target).await {
            Ok(lk) => lk.closest.into_iter().find(|c| c.node_id == target),
            Err(e) => { warn!("lookup {} failed: {e}", target.short()); None }
        };
        self.tunnels.get_or_build_with(&target, found.as_ref()).await?;
        let pk = match (pubkey_hint, &found) {
            (Some(k), _) => k.to_vec(),
            (None, Some(c)) => c.identity_public_key.clone(),
            (None, None) => self.recipient_pubkey(&target).await?,
        };
        // Relay видят только Envelope
        let data = crate::security::e2e::seal(&target, &pk, &encode(msg)?)?;
        self.tunnels.send(&target, data).await
    }

    /// Отправить текстовое сообщение узлу `target` (без ожидания подтверждения).
    pub async fn send_text(&self, target: NodeId, text: &str) -> Result<()> {
        let msg = self.build_text(target, text)?;
        self.deliver(target, None, &msg).await?;
        info!("→ msg {} to {} ({} bytes)", msg.message_id_hex(), target.short(), text.len());
        Ok(())
    }

    /// Зарегистрировать ожидание APP_ACK на сообщение `id`, отправленное узлу `target`.
    pub async fn register_ack_wait(&self, id: Vec<u8>, target: NodeId) -> oneshot::Receiver<()> {
        let (tx, rx) = oneshot::channel();
        self.waiting.lock().await.insert(id, (target, tx));
        rx
    }

    /// Отправить и дождаться APP_ACK от получателя. Ok(rtt_ms) — сообщение доставлено и прочитано.
    pub async fn send_text_acked(&self, target: NodeId, text: &str, wait: std::time::Duration) -> Result<u64> {
        let msg = self.build_text(target, text)?;
        let id = msg.message_id.clone();
        let rx = self.register_ack_wait(id.clone(), target).await;
        let t0 = now_ms();
        if let Err(e) = self.deliver(target, None, &msg).await {
            self.waiting.lock().await.remove(&id);
            return Err(e);
        }
        info!("→ msg {} to {} ({} bytes), ждём APP_ACK", msg.message_id_hex(), target.short(), text.len());
        match tokio::time::timeout(wait, rx).await {
            Ok(Ok(())) => Ok(now_ms().saturating_sub(t0)),
            _ => {
                self.waiting.lock().await.remove(&id);
                bail!("нет подтверждения доставки за {} мс", wait.as_millis())
            }
        }
    }

    /// Отправить APP_ACK на принятое сообщение (в фоне; сбой не влияет на приём).
    async fn send_ack(&self, to: NodeId, to_pubkey: Vec<u8>, acked_id: Vec<u8>) {
        let mut ack = AppMessage::new_ack(self.identity.node_id, to, &acked_id);
        self.sign(&mut ack);
        if let Err(e) = self.deliver(to, Some(&to_pubkey), &ack).await {
            warn!("APP_ACK to {} не отправлен: {e}", to.short());
        }
    }

    /// Диагностика маршрута до `target` (туннель, цепочка relay, живость hop).
    pub async fn route_diag(&self, target: &NodeId) -> serde_json::Value {
        let in_table = self.dht.routing.lock().await
            .find_closest(target, self.cfg.dht.k_bucket_size.max(8))
            .iter().any(|c| c.node_id == *target);
        serde_json::json!({
            "target": target.to_hex(),
            "in_routing_table": in_table,
            "tunnel": self.tunnels.route_json(target).await,
        })
    }

    /// Обработать входящий TunnelMessage → проверить подпись → положить в inbox.
    pub async fn handle_incoming(self: &Arc<Self>, raw: Vec<u8>) -> Result<()> {
        // E2E: открытый (нешифрованный) AppMessage не принимаем
        let plain = crate::security::e2e::open(&self.identity, &raw)?;
        let msg: AppMessage = decode(&plain)
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

        match msg.kind {
            // APP_ACK: будим отправителя, только если подтверждение пришло от адресата
            MessageKind::Ack => {
                let mut w = self.waiting.lock().await;
                match w.get(&msg.payload) {
                    Some((target, _)) if *target == msg.from => {
                        if let Some((_, tx)) = w.remove(&msg.payload) { let _ = tx.send(()); }
                    }
                    Some(_) => warn!("APP_ACK от неожиданного узла {} отброшен", msg.from.short()),
                    None => tracing::debug!("APP_ACK на неизвестное сообщение"),
                }
                return Ok(());
            }
            MessageKind::Text => {
                let (from, pk, id) = (msg.from, msg.sender_pubkey.clone(), msg.message_id.clone());
                {
                    let mut inbox = self.inbox.lock().await;
                    if inbox.len() >= 100 { inbox.pop_front(); }
                    inbox.push_back(msg);
                }
                let me = self.clone();
                tokio::spawn(async move { me.send_ack(from, pk, id).await });
            }
            _ => {
                let mut inbox = self.inbox.lock().await;
                if inbox.len() >= 100 { inbox.pop_front(); }
                inbox.push_back(msg);
            }
        }
        Ok(())
    }

    /// Публичный ключ получателя: из таблицы маршрутизации, иначе из его NodeRecord в DHT.
    async fn recipient_pubkey(&self, target: &NodeId) -> Result<Vec<u8>> {
        let known = self.dht.routing.lock().await
            .find_closest(target, self.cfg.dht.k_bucket_size.max(8))
            .into_iter().find(|c| c.node_id == *target)
            .map(|c| c.identity_public_key);
        if let Some(k) = known { return Ok(k); }
        Ok(self.dht.resolve_node(target).await
            .map_err(|e| anyhow::anyhow!("ключ получателя неизвестен: {e}"))?
            .identity_pubkey.to_vec())
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
