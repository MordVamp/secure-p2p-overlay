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

use crate::app::files::{BlockOutcome, FileAck, FileBlock, FileDone, FileOffer, FileStore, TransferInfo, safe_name, sha256_file, MAX_BLOCK};
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
    /// Размер отправленного E2E-конверта, байт (для доли служебного трафика).
    pub wire_bytes:      u64,
    /// Основной туннель отказал, доставлено по резервному из пула (продвинутый уровень).
    pub failover:        bool,
    /// От начала передачи до обнаружения отказа, мс (если отказ был).
    pub detect_ms:       Option<u64>,
    /// От обнаружения отказа до TUNNEL_ACK по резервному туннелю, мс.
    pub switch_ms:       Option<u64>,
}

/// Итог передачи файла (§13.2, метрики §15.2: goodput и доля служебного трафика).
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct FileReport {
    pub transfer_id:  String,
    pub name:         String,
    pub size:         u64,
    pub blocks:       u32,
    pub duration_ms:  u64,
    /// Полезная скорость: размер файла / время от предложения до подтверждения SHA-256 получателем, МиБ/с.
    pub goodput_mib_s: f64,
    /// Байты конвертов отправителя (предложение, блоки, повторы) и полученных подтверждений.
    pub bytes_out:    u64,
    pub bytes_in:     u64,
    pub messages_out: u32,
    pub retransmits:  u32,
    /// (bytes_out + bytes_in − size) / (bytes_out + bytes_in) — служебная доля на прикладном уровне.
    pub overhead_share: f64,
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
    /// Анти-replay: окно по подписанной метке времени + принятые message_id (на диске — с `with_replay_log`).
    seen:       Mutex<crate::app::replay::ReplayGuard>,
    /// APP_ACK: message_id → (кому отправили, канал пробуждения отправителя).
    waiting:    Mutex<HashMap<Vec<u8>, (NodeId, oneshot::Sender<()>)>>,
    /// Проверенные ключи собеседников (NodeID = hash(ключ), подделать нельзя).
    keys:       Mutex<HashMap<NodeId, Vec<u8>>>,
    /// §13.1: локальная история отправки и приёма (None — не ведётся, напр. в тестах).
    history:    Option<History>,
    /// §13.2: приём файлов, ожидающие подтверждений отправители и сводка передач.
    files:      Mutex<FileStore>,
    file_waiters: Mutex<HashMap<Vec<u8>, (NodeId, mpsc::Sender<(AppMessage, usize)>)>>,
    transfers:  Mutex<VecDeque<TransferInfo>>,
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
        let files = FileStore::new(&cfg.node.state_dir, cfg.files.max_bytes, cfg.files.incomplete_timeout_s * 1000);
        Self { identity, dht, tunnels, forwarder, metrics, cfg, inbox: Mutex::new(VecDeque::new()),
               seen: Mutex::new(crate::app::replay::ReplayGuard::in_memory()), waiting: Mutex::new(Default::default()),
               keys: Mutex::new(Default::default()), history: None,
               files: Mutex::new(files),
               file_waiters: Mutex::new(Default::default()), transfers: Mutex::new(VecDeque::new()) }
    }

    /// Хранить принятые message_id в `<state_dir>/replay.log` — повтор отвергается и после перезапуска.
    pub fn with_replay_log(self, state_dir: &std::path::Path) -> Self {
        Self { seen: Mutex::new(crate::app::replay::ReplayGuard::persistent(state_dir)), ..self }
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
        rep.wire_bytes = data.len() as u64;

        let t0 = now_ms();
        match self.tunnels.send(&target, data.clone()).await {
            Ok(o) => {
                rep.tunnel_send_ms = now_ms().saturating_sub(t0);
                (rep.failover, rep.detect_ms, rep.switch_ms) = (o.failover, o.detect_ms, o.switch_ms);
            }
            Err(e) => {
                // §12: пригодных туннелей не осталось — свежий lookup, новый маршрут без отказавшего relay, повтор
                warn!("tunnel to {} failed: {e}; перестраиваем маршрут", target.short());
                rep.rebuilt_after_failure = true;
                rep.detect_ms = Some(now_ms().saturating_sub(t0));
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
        let raw_len = raw.len();
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

        // Анти-replay: окно по подписанной метке времени + уже принятые message_id
        if let Err(e) = self.seen.lock().await.check(&msg.message_id, msg.timestamp_ms) {
            bail!("duplicate message {}: {e}", msg.message_id_hex());
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
            MessageKind::FileOffer | MessageKind::FileBlock => {
                self.on_file_message(msg).await;
                return Ok(());
            }
            MessageKind::FileAck | MessageKind::FileDone => {
                // подтверждения — ожидающему отправителю, только если пришли от получателя файла
                let tid = match msg.kind {
                    MessageKind::FileAck => decode::<FileAck>(&msg.payload).map(|a| a.transfer_id),
                    _ => decode::<FileDone>(&msg.payload).map(|d| d.transfer_id),
                };
                let Ok(tid) = tid else { bail!("некорректное подтверждение файла") };
                let w = self.file_waiters.lock().await;
                match w.get(&tid) {
                    Some((target, tx)) if *target == msg.from => { let _ = tx.try_send((msg, raw_len)); }
                    Some(_) => warn!("подтверждение файла от неожиданного узла {} отброшено", msg.from.short()),
                    None => tracing::debug!("подтверждение неизвестной передачи"),
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

    // ── §13.2: передача файлов ─────────────────────────────────────────────

    fn build_kind<T: serde::Serialize>(&self, target: NodeId, kind: MessageKind, body: &T) -> Result<AppMessage> {
        let mut m = AppMessage::new_kind(self.identity.node_id, target, kind, encode(body)?);
        self.sign(&mut m);
        Ok(m)
    }

    async fn upsert_transfer(&self, t: TransferInfo) {
        let mut v = self.transfers.lock().await;
        match v.iter_mut().find(|x| x.transfer_id == t.transfer_id) {
            Some(x) => *x = t,
            None => { if v.len() >= 50 { v.pop_front(); } v.push_back(t); }
        }
    }

    async fn update_transfer(&self, id: &str, f: impl FnOnce(&mut TransferInfo)) {
        if let Some(x) = self.transfers.lock().await.iter_mut().find(|x| x.transfer_id == id) { f(x); }
    }

    /// Сводка передач (последние 50) и каталог загрузок.
    pub async fn transfers(&self) -> (Vec<TransferInfo>, std::path::PathBuf) {
        (self.transfers.lock().await.iter().cloned().collect(), self.files.lock().await.dir.clone())
    }

    /// Удалить неполные входящие файлы по тайм-ауту (вызывается периодически).
    pub async fn evict_stale_files(&self) {
        let stale = self.files.lock().await.evict_stale();
        for id in stale {
            let hid = hex::encode(&id);
            warn!("файл {}: неполный, удалён по тайм-ауту", &hid[..8]);
            self.update_transfer(&hid, |t| { t.status = "failed".into(); t.error = Some("тайм-аут: файл не получен целиком".into()); t.finished_ms = Some(now_ms()); }).await;
        }
    }

    fn file_reply<T: serde::Serialize + Send + 'static>(self: &Arc<Self>, to: NodeId, pk: Vec<u8>, kind: MessageKind, body: T) {
        let me = self.clone();
        tokio::spawn(async move {
            let Ok(m) = me.build_kind(to, kind, &body) else { return };
            if let Err(e) = me.deliver(to, Some(&pk), &m).await { warn!("ответ по файлу {} не отправлен: {e}", to.short()); }
        });
    }

    /// Получатель: FileOffer / FileBlock.
    async fn on_file_message(self: &Arc<Self>, msg: AppMessage) {
        let (from, pk) = (msg.from, msg.sender_pubkey.clone());
        let window = self.cfg.files.window.max(1) as u32;
        match msg.kind {
            MessageKind::FileOffer => {
                let Ok(o) = decode::<FileOffer>(&msg.payload) else { return };
                let tid = o.transfer_id.clone();
                let info = TransferInfo {
                    transfer_id: hex::encode(&tid), dir: "in".into(), peer: from.to_hex(), name: o.name.clone(), size: o.size,
                    blocks: o.blocks, done_blocks: 0, status: "receiving".into(), error: None, path: None,
                    started_ms: now_ms(), finished_ms: None,
                };
                let res = self.files.lock().await.on_offer(from, o);
                match res {
                    Ok(()) => { info!("← файл {} от {}: {} байт", info.name, from.short(), info.size); self.upsert_transfer(info).await; }
                    Err(e) => {
                        warn!("предложение файла от {} отклонено: {e}", from.short());
                        self.upsert_transfer(TransferInfo { status: "rejected".into(), error: Some(e.to_string()), finished_ms: Some(now_ms()), ..info }).await;
                        self.file_reply(from, pk, MessageKind::FileDone, FileDone { transfer_id: tid, ok: false, error: Some(e.to_string()) });
                    }
                }
            }
            MessageKind::FileBlock => {
                let Ok(b) = decode::<FileBlock>(&msg.payload) else { return };
                let hid = hex::encode(&b.transfer_id);
                let res = self.files.lock().await.on_block(from, &b);
                let outcome = match res {
                    Ok(o) => o,
                    Err(e) => { tracing::debug!("блок файла отброшен: {e}"); return; }
                };
                let (done, blocks, _) = self.files.lock().await.progress(&b.transfer_id).unwrap_or((0, 0, 0));
                self.update_transfer(&hid, |t| t.done_blocks = done).await;
                let window_end = (b.index + 1) % window == 0 || b.index + 1 == blocks;
                if outcome == BlockOutcome::Complete {
                    let res = self.files.lock().await.finish(&b.transfer_id);
                    let (ok, err, path) = match res {
                        Ok(p) => (true, None, Some(p.display().to_string())),
                        Err(e) => (false, Some(e.to_string()), None),
                    };
                    self.update_transfer(&hid, |t| { t.done_blocks = t.blocks; t.status = if ok { "received".into() } else { "failed".into() };
                                                     t.error = err.clone(); t.path = path.clone(); t.finished_ms = Some(now_ms()); }).await;
                    if let Some(h) = &self.history {
                        let name = self.transfers.lock().await.iter().find(|t| t.transfer_id == hid).map(|t| t.name.clone()).unwrap_or_default();
                        h.append(&HistoryEntry { ts_ms: now_ms(), dir: "in".into(), peer: from.to_hex(), message_id: hid.clone(),
                            status: if ok { "received".into() } else { "failed".into() }, text: Some(format!("📎 {name}")),
                            rtt_ms: None, error: err.clone(), lookup_ms: None, tunnel_build_ms: None, tunnel_send_ms: None }).await;
                    }
                    // FileAck последнего окна, затем FileDone — по порядку, в одной задаче
                    let ack = FileAck { transfer_id: b.transfer_id.clone(), received: blocks, missing: vec![] };
                    let done = FileDone { transfer_id: b.transfer_id, ok, error: err };
                    let me = self.clone();
                    tokio::spawn(async move {
                        for (kind, body) in [(MessageKind::FileAck, encode(&ack)), (MessageKind::FileDone, encode(&done))] {
                            let Ok(body) = body else { return };
                            let mut m = AppMessage::new_kind(me.identity.node_id, from, kind, body);
                            me.sign(&mut m);
                            if let Err(e) = me.deliver(from, Some(&pk), &m).await { warn!("ответ по файлу {} не отправлен: {e}", from.short()); }
                        }
                    });
                } else if window_end {
                    if let Some(ack) = self.files.lock().await.ack(&b.transfer_id, b.index + 1) {
                        self.file_reply(from, pk, MessageKind::FileAck, ack);
                    }
                }
            }
            _ => {}
        }
    }

    /// Отправить файл: предложение → блоки окнами с подтверждением → повтор пропущенных → итог от получателя.
    pub async fn send_file(self: &Arc<Self>, target: NodeId, path: &std::path::Path) -> Result<FileReport> {
        use std::io::{Read, Seek, SeekFrom};
        let size = std::fs::metadata(path).map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?.len();
        if size > self.cfg.files.max_bytes { bail!("файл {size} байт больше лимита {} байт", self.cfg.files.max_bytes); }
        let name = safe_name(&path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default())?;
        let sha256 = sha256_file(path)?;
        let bs = self.cfg.files.block_size.clamp(1024, MAX_BLOCK) as u64;
        let blocks = size.div_ceil(bs).max(1) as u32;
        let window = self.cfg.files.window.max(1) as u32;
        let tid = uuid::Uuid::new_v4().into_bytes().to_vec();
        let hid = hex::encode(&tid);
        let (tx, mut rx) = mpsc::channel(256);
        self.file_waiters.lock().await.insert(tid.clone(), (target, tx));
        let t0 = now_ms();
        self.upsert_transfer(TransferInfo { transfer_id: hid.clone(), dir: "out".into(), peer: target.to_hex(), name: name.clone(),
            size, blocks, done_blocks: 0, status: "sending".into(), error: None, path: Some(path.display().to_string()),
            started_ms: t0, finished_ms: None }).await;
        let mut rep = FileReport { transfer_id: hid.clone(), name: name.clone(), size, blocks, ..Default::default() };

        let res: Result<()> = async {
            let offer = FileOffer { transfer_id: tid.clone(), name: name.clone(), size, sha256, block_size: bs as u32, blocks };
            let m = self.build_kind(target, MessageKind::FileOffer, &offer)?;
            rep.bytes_out += self.deliver(target, None, &m).await?.wire_bytes;
            rep.messages_out += 1;
            let mut f = std::fs::File::open(path)?;
            let mut next = 0u32;
            let mut done_ok = false;   // итог мог прийти раньше подтверждения последнего окна
            while next < blocks {
                let end = (next + window).min(blocks);
                let mut pending: Vec<u32> = (next..end).collect();
                let mut rounds = 0;
                loop {
                    // последний блок окна шлётся в каждом раунде: по нему получатель отвечает FileAck
                    if !pending.contains(&(end - 1)) { pending.push(end - 1); }
                    for &i in &pending {
                        let off = i as u64 * bs;
                        let len = (size - off).min(bs) as usize;
                        let mut data = vec![0u8; len];
                        f.seek(SeekFrom::Start(off))?;
                        f.read_exact(&mut data)?;
                        let m = self.build_kind(target, MessageKind::FileBlock, &FileBlock { transfer_id: tid.clone(), index: i, data })?;
                        rep.bytes_out += self.deliver(target, None, &m).await?.wire_bytes;
                        rep.messages_out += 1;
                    }
                    if rounds > 0 { rep.retransmits += pending.len() as u32; }
                    self.update_transfer(&hid, |t| t.done_blocks = end).await;
                    // ждём подтверждение окна (или отказ получателя)
                    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
                    let mut missing: Option<Vec<u32>> = None;
                    while missing.is_none() {
                        match tokio::time::timeout_at(deadline, rx.recv()).await {
                            Ok(Some((m, n))) => {
                                rep.bytes_in += n as u64;
                                if m.kind == MessageKind::FileDone {
                                    let d: FileDone = decode(&m.payload)?;
                                    if !d.ok { bail!("получатель отказал: {}", d.error.unwrap_or_default()); }
                                    done_ok = true;
                                    if next + window >= blocks { missing = Some(vec![]); }
                                } else if let Ok(a) = decode::<FileAck>(&m.payload) {
                                    if a.received >= end || a.missing.iter().all(|&x| x < end) {
                                        missing = Some(a.missing.into_iter().filter(|&x| x >= next && x < end).collect());
                                    }
                                }
                            }
                            _ => break,
                        }
                    }
                    rounds += 1;
                    match missing {
                        Some(m) if m.is_empty() => break,
                        Some(m) => pending = m,
                        None => pending = (next..end).collect(),   // подтверждения нет — повторить окно
                    }
                    if rounds > 3 { bail!("окно блоков {next}..{end} не подтверждено после 3 повторов"); }
                }
                next = end;
            }
            // итог: получатель проверил SHA-256
            if done_ok { return Ok(()); }
            let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(20);
            loop {
                match tokio::time::timeout_at(deadline, rx.recv()).await {
                    Ok(Some((m, n))) => {
                        rep.bytes_in += n as u64;
                        if m.kind == MessageKind::FileDone {
                            let d: FileDone = decode(&m.payload)?;
                            if d.ok { return Ok(()); }
                            bail!("получатель отклонил файл: {}", d.error.unwrap_or_default());
                        }
                    }
                    _ => bail!("нет итогового подтверждения (SHA-256) от получателя"),
                }
            }
        }.await;

        self.file_waiters.lock().await.remove(&tid);
        rep.duration_ms = now_ms().saturating_sub(t0).max(1);
        rep.goodput_mib_s = size as f64 / (1024.0 * 1024.0) / (rep.duration_ms as f64 / 1000.0);
        let total = (rep.bytes_out + rep.bytes_in) as f64;
        rep.overhead_share = if total > 0.0 { ((total - size as f64) / total).max(0.0) } else { 0.0 };
        let (status, err) = match &res { Ok(()) => ("delivered", None), Err(e) => ("failed", Some(e.to_string())) };
        self.update_transfer(&hid, |t| { t.status = if res.is_ok() { "sent".into() } else { "failed".into() };
                                         t.error = err.clone(); t.finished_ms = Some(now_ms()); if res.is_ok() { t.done_blocks = t.blocks; } }).await;
        if let Some(h) = &self.history {
            h.append(&HistoryEntry { ts_ms: now_ms(), dir: "out".into(), peer: target.to_hex(), message_id: hid.clone(),
                status: status.into(), text: Some(format!("📎 {name}")), rtt_ms: Some(rep.duration_ms), error: err,
                lookup_ms: None, tunnel_build_ms: None, tunnel_send_ms: None }).await;
        }
        res.map(|_| rep)
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
