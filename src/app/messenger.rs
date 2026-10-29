//! app/messenger.rs — Высокоуровневый API отправки/приёма E2E сообщений.
//!
//! Шаги отправки:
//!  1. find_node(target) — получить адрес (пропускается, если туннель уже
//!     установлен и ключ получателя известен — «установленная сессия», §15.2)
//!  2. get_or_build_tunnel(target) — построить туннель
//!  3. sign(message) — Ed25519 подпись, затем E2E-шифрование (security/e2e.rs)
//!  4. serialize → send_data через TunnelForwarder, ожидание TUNNEL_ACK;
//!     при отказе маршрута — новый lookup, новый туннель и один повтор (§12)
//!
//! Шаги приёма:
//!  1. TunnelForwarder.inbox → TunnelMessage
//!  2. deserialize AppMessage
//!  3. verify signature (Ed25519 + NodeId соответствует pubkey)
//!  4. доставить в приложение через mpsc

use std::sync::Arc;
use std::collections::{HashMap, VecDeque};

use anyhow::{Result, bail};
use tokio::sync::{Mutex, mpsc, oneshot};
use tracing::{info, warn};

use crate::app::history::{History, HistoryEntry};
use crate::app::message::{AppMessage, MessageKind};
use crate::config::NodeConfig;
use crate::dht::node::DhtNode;
use crate::identity::node_identity::NodeIdentity;
use crate::metrics::MetricsCollector;
use crate::protocol::payload::{encode, decode};
use crate::tunnel::manager::TunnelManager;
use crate::tunnel::forwarder::TunnelForwarder;
use crate::types::{Contact, NodeId, now_ms};

/// Входящие подтверждённые сообщения для UI.
pub type MessageRx = mpsc::Receiver<AppMessage>;

/// Разбивка времени одной отправки (§15.2: первое сообщение и установленная сессия раздельно).
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct SendReport {
    /// message_id отправленного сообщения (hex).
    pub message_id:      String,
    /// Lookup получателя; None — не понадобился (туннель и ключ уже были).
    pub lookup_ms:       Option<u64>,
    /// Построение туннеля; None — использован уже установленный.
    pub tunnel_build_ms: Option<u64>,
    /// От передачи первому relay до TUNNEL_ACK: relay подтверждает только после
    /// следующего hop, т.е. это время прохода кадра по цепочке до получателя и обратно.
    pub tunnel_send_ms:  u64,
    /// Маршрут отказал во время этой отправки и был перестроен.
    pub rebuilt_after_failure: bool,
    /// Полное время до APP_ACK (заполняет send_text_acked).
    pub rtt_ms:          u64,
}

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
    waiting:    Mutex<HashMap<Vec<u8>, (NodeId, oneshot::Sender<()>)>>,
    /// Проверенные ключи собеседников (NodeID = hash(ключ), подделать нельзя).
    keys:       Mutex<HashMap<NodeId, Vec<u8>>>,
    /// §13.1: локальная история отправки и приёма (None — не ведётся, напр. в тестах).
    history:    Option<History>,
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
        Self { identity, dht, tunnels, forwarder, metrics, cfg, inbox: Mutex::new(VecDeque::new()),
               seen: Mutex::new(Default::default()), waiting: Mutex::new(Default::default()),
               keys: Mutex::new(Default::default()), history: None }
    }

    /// Вести историю в `<state_dir>/history.jsonl`.
    pub fn with_history(mut self, h: History) -> Self { self.history = Some(h); self }

    /// Последние `limit` записей истории.
    pub async fn history(&self, limit: usize) -> Vec<HistoryEntry> {
        match &self.history { Some(h) => h.tail(limit).await, None => vec![] }
    }

    async fn log(&self, dir: &str, peer: &NodeId, msg: &AppMessage, status: &str,
                 rep: Option<&SendReport>, error: Option<String>) {
        let Some(h) = &self.history else { return };
        h.append(&HistoryEntry {
            ts_ms: now_ms(), dir: dir.into(), peer: peer.to_hex(),
            message_id: hex::encode(&msg.message_id), status: status.into(),
            text: msg.text().map(str::to_owned), error,
            rtt_ms:          rep.map(|r| r.rtt_ms),
            lookup_ms:       rep.and_then(|r| r.lookup_ms),
            tunnel_build_ms: rep.and_then(|r| r.tunnel_build_ms),
            tunnel_send_ms:  rep.map(|r| r.tunnel_send_ms),
        }).await;
    }

    /// Запомнить ключ собеседника, если он действительно соответствует его NodeID.
    async fn remember_key(&self, id: NodeId, key: &[u8]) {
        if !crate::identity::node_identity::verify_node_id_bytes(&id, key) { return; }
        let mut m = self.keys.lock().await;
        if m.len() >= 4096 { m.clear(); }
        m.insert(id, key.to_vec());
    }

    async fn known_key(&self, id: &NodeId) -> Option<Vec<u8>> {
        self.keys.lock().await.get(id).cloned()
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

    /// Найти контакт получателя итеративным lookup. Возвращает (контакт, время в мс).
    /// Контакт берём из результата lookup, а не только из таблицы маршрутизации:
    /// при малом k корзина может быть полна и не удержать найденный узел.
    async fn locate(&self, target: &NodeId) -> (Option<Contact>, u64) {
        let t0 = now_ms();
        let found = match self.dht.find_node(target).await {
            Ok(lk) => lk.closest.into_iter().find(|c| c.node_id == *target),
            Err(e) => { warn!("lookup {} failed: {e}", target.short()); None }
        };
        if let Some(c) = &found { self.remember_key(*target, &c.identity_public_key).await; }
        (found, now_ms().saturating_sub(t0))
    }

    /// Доставить подписанное сообщение: lookup → туннель → E2E-шифрование → отправка.
    /// `pubkey_hint` — ключ получателя, если он уже известен (например, из входящего сообщения).
    async fn deliver(&self, target: NodeId, pubkey_hint: Option<&[u8]>, msg: &AppMessage) -> Result<SendReport> {
        let mut rep = SendReport { message_id: hex::encode(&msg.message_id), ..Default::default() };
        if let Some(k) = pubkey_hint { self.remember_key(target, k).await; }

        // Установленная сессия: туннель жив и ключ известен — lookup не нужен
        let mut found = None;
        if !(self.tunnels.is_usable(&target).await && self.known_key(&target).await.is_some()) {
            let (c, ms) = self.locate(&target).await;
            found = c;
            rep.lookup_ms = Some(ms);
        }
        // A5: последний relay должен знать адрес получателя
        rep.tunnel_build_ms = self.tunnels.get_or_build_with(&target, found.as_ref()).await?;
        let pk = match self.known_key(&target).await {
            Some(k) => k,
            None => {
                let k = self.recipient_pubkey(&target).await?;
                self.remember_key(target, &k).await;
                k
            }
        };
        // Relay видят только Envelope
        let data = crate::security::e2e::seal(&target, &pk, &encode(msg)?)?;

        let t0 = now_ms();
        match self.tunnels.send(&target, data.clone()).await {
            Ok(()) => rep.tunnel_send_ms = now_ms().saturating_sub(t0),
            Err(e) => {
                // §12: туннель помечен DEAD — свежий lookup, новый маршрут без отказавшего relay, повтор
                warn!("tunnel to {} failed: {e}; перестраиваем маршрут", target.short());
                rep.rebuilt_after_failure = true;
                let (c, ms) = self.locate(&target).await;
                rep.lookup_ms = Some(rep.lookup_ms.unwrap_or(0) + ms);
                let built = self.tunnels.get_or_build_with(&target, c.as_ref()).await?;
                rep.tunnel_build_ms = Some(rep.tunnel_build_ms.unwrap_or(0) + built.unwrap_or(0));
                let t1 = now_ms();
                self.tunnels.send(&target, data).await?;
                rep.tunnel_send_ms = now_ms().saturating_sub(t1);
            }
        }
        Ok(rep)
    }

    /// Отправить текстовое сообщение узлу `target` (без ожидания подтверждения).
    pub async fn send_text(&self, target: NodeId, text: &str) -> Result<()> {
        let msg = self.build_text(target, text)?;
        if let Err(e) = self.deliver(target, None, &msg).await {
            self.log("out", &target, &msg, "failed", None, Some(e.to_string())).await;
            return Err(e);
        }
        info!("→ msg {} to {} ({} bytes)", msg.message_id_hex(), target.short(), text.len());
        self.log("out", &target, &msg, "sent", None, None).await;
        Ok(())
    }

    /// Зарегистрировать ожидание APP_ACK на сообщение `id`, отправленное узлу `target`.
    pub async fn register_ack_wait(&self, id: Vec<u8>, target: NodeId) -> oneshot::Receiver<()> {
        let (tx, rx) = oneshot::channel();
        self.waiting.lock().await.insert(id, (target, tx));
        rx
    }

    /// Отправить и дождаться APP_ACK от получателя. Ok — сообщение доставлено и прочитано.
    pub async fn send_text_acked(&self, target: NodeId, text: &str, wait: std::time::Duration) -> Result<SendReport> {
        let msg = self.build_text(target, text)?;
        let id = msg.message_id.clone();
        let rx = self.register_ack_wait(id.clone(), target).await;
        let t0 = now_ms();
        let mut rep = match self.deliver(target, None, &msg).await {
            Ok(r) => r,
            Err(e) => {
                self.waiting.lock().await.remove(&id);
                self.log("out", &target, &msg, "failed", None, Some(e.to_string())).await;
                return Err(e);
            }
        };
        info!("→ msg {} to {} ({} bytes), ждём APP_ACK", msg.message_id_hex(), target.short(), text.len());
        match tokio::time::timeout(wait, rx).await {
            Ok(Ok(())) => {
                rep.rtt_ms = now_ms().saturating_sub(t0);
                self.log("out", &target, &msg, "delivered", Some(&rep), None).await;
                Ok(rep)
            }
            _ => {
                self.waiting.lock().await.remove(&id);
                let err = format!("нет подтверждения доставки за {} мс", wait.as_millis());
                self.log("out", &target, &msg, "failed", None, Some(err.clone())).await;
                bail!(err)
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
        self.remember_key(msg.from, &msg.sender_pubkey).await;

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
                self.log("in", &from, &msg, "received", None, None).await;
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
