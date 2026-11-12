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
use crate::app::calls::Call;

mod call_ops;
use crate::app::events::EventBus;
use crate::app::mailbox::{self, MailboxBody, Outbox, Queued};
use crate::app::message::{AppMessage, GroupBody, MessageKind, ProbeBody, ReadBody};

/// Время в микросекундах (для измерительных пакетов: на 20-мс потоке миллисекунд мало).
pub fn now_us() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_micros() as u64).unwrap_or(0)
}

/// Принятый измерительный пакет.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ProbeRx { pub stream: String, pub seq: u32, pub sent_us: u64, pub recv_us: u64, pub from: String }
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
    /// Не доставлено через туннель — поставлено в почтовый ящик DHT (офлайн-доставка).
    pub queued:          bool,
    /// Причина, по которой сообщение ушло в ящик.
    pub queued_reason:   Option<String>,
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
    /// События для клиентов (push, `/api/events`).
    pub events: Arc<EventBus>,
    /// Офлайн-доставка: очередь неотправленного и дополнительные собеседники для проверки ящиков.
    outbox:     Mutex<Outbox>,
    watch:      Mutex<std::collections::HashSet<NodeId>>,
    /// «Печатает»: собеседник → до какого момента показывать.
    typing:     Mutex<HashMap<NodeId, u64>>,
    /// Принятые измерительные пакеты (до 50 000).
    probes:     Mutex<VecDeque<ProbeRx>>,
    /// Звонки: call_id → состояние, ключи, статистика приёма.
    calls:      Mutex<HashMap<Vec<u8>, Call>>,
    /// Звук: кодер исходящего потока; локальный аудиоканал к клиенту (порт и токен).
    voice:      std::sync::Mutex<call_ops::VoiceState>,
    audio_link: tokio::sync::OnceCell<(u16, [u8; crate::app::voice::TOKEN_LEN])>,
}

/// Результат групповой рассылки по одному участнику.
#[derive(Debug, Clone, serde::Serialize)]
pub struct GroupResult {
    pub member: String,
    /// delivered | queued | failed
    pub status: String,
    pub error:  Option<String>,
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
               file_waiters: Mutex::new(Default::default()), transfers: Mutex::new(VecDeque::new()),
               events: Arc::new(EventBus::new()), outbox: Mutex::new(Outbox::in_memory()),
               watch: Mutex::new(Default::default()), typing: Mutex::new(Default::default()),
               probes: Mutex::new(VecDeque::new()), calls: Mutex::new(HashMap::new()),
               voice: Default::default(), audio_link: tokio::sync::OnceCell::new() }
    }

    /// Хранить очередь офлайн-доставки в `<state_dir>/outbox.json` (переживает перезапуск).
    pub fn with_outbox(self, state_dir: &std::path::Path) -> Self {
        Self { outbox: Mutex::new(Outbox::load(state_dir)), ..self }
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
            group: None, group_name: None, members: None, read_ms: None,
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
                return self.fail_or_queue(target, &msg, e.to_string()).await;
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
                self.fail_or_queue(target, &msg, format!("нет подтверждения доставки за {} мс", wait.as_millis())).await
            }
        }
    }

    /// Не доставлено: при включённой офлайн-доставке — в почтовый ящик DHT (статус queued), иначе ошибка.
    async fn fail_or_queue(&self, target: NodeId, msg: &AppMessage, err: String) -> Result<SendReport> {
        if self.cfg.mailbox.enabled {
            match self.queue_offline(target, msg).await {
                Ok(()) => {
                    let why = format!("{err}; сообщение в почтовом ящике DHT — получатель заберёт его сам");
                    self.log("out", &target, msg, "queued", None, Some(why.clone())).await;
                    return Ok(SendReport { message_id: hex::encode(&msg.message_id), queued: true,
                                           queued_reason: Some(err), ..Default::default() });
                }
                Err(e) => warn!("офлайн-очередь к {}: {e}", target.short()),
            }
        }
        self.log("out", &target, msg, "failed", None, Some(err.clone())).await;
        bail!(err)
    }

    // ── офлайн-доставка (почтовые ящики DHT) ────────────────────────────────

    /// Зашифровать сообщение для получателя и положить в его ящик.
    async fn queue_offline(&self, target: NodeId, msg: &AppMessage) -> Result<()> {
        let pk = match self.known_key(&target).await {
            Some(k) => k,
            None => self.recipient_pubkey(&target).await?,
        };
        let env = crate::security::e2e::seal(&target, &pk, &encode(msg)?)?;
        self.outbox.lock().await.push(&target, Queued {
            id: hex::encode(&msg.message_id), ts_ms: msg.timestamp_ms, envelope: hex::encode(env),
        }, self.cfg.mailbox.max_messages);
        self.publish_mailbox(target).await;
        info!("msg {} → почтовый ящик DHT для {}", msg.message_id_hex(), target.short());
        Ok(())
    }

    /// Опубликовать (обновить) ящик для `to`. Ошибка кворума не фатальна: запись осталась
    /// локально и уйдёт на реплики при republish.
    async fn publish_mailbox(&self, to: NodeId) {
        let body = self.outbox.lock().await.body(&to);
        let Ok(v) = encode(&body) else { return };
        if let Err(e) = self.dht.put(&self.identity, mailbox::key(&to, &self.identity.node_id), v,
                                     self.cfg.dht.ttl_seconds).await {
            warn!("почтовый ящик для {}: {e}", to.short());
        }
    }

    /// После перезапуска: заново опубликовать ящики из сохранённой очереди.
    pub async fn republish_outbox(&self) {
        let rs = self.outbox.lock().await.recipients();
        for r in rs {
            let Ok(b) = hex::decode(&r) else { continue };
            let Ok(arr) = <[u8; 32]>::try_from(b.as_slice()) else { continue };
            self.publish_mailbox(NodeId(arr)).await;
        }
    }

    /// Проверять ящик этого собеседника (кроме тех, с кем уже есть переписка).
    pub async fn watch_peer(&self, id: NodeId) { self.watch.lock().await.insert(id); }

    /// Очередь офлайн-доставки: получатель → число сообщений.
    pub async fn outbox_summary(&self) -> std::collections::BTreeMap<String, usize> {
        self.outbox.lock().await.summary()
    }

    /// Выбросить из очереди сообщения старше `mailbox.max_age_s` (статус failed).
    pub async fn expire_outbox(&self) {
        let gone = self.outbox.lock().await.expire(self.cfg.mailbox.max_age_s * 1000);
        for (to, ids) in gone {
            let Ok(b) = hex::decode(&to) else { continue };
            let Ok(arr) = <[u8; 32]>::try_from(b.as_slice()) else { continue };
            let id = NodeId(arr);
            if let Some(h) = &self.history {
                for m in &ids { h.append(&HistoryEntry::status_update(&to, m, Some("failed"), None)).await; }
            }
            self.publish_mailbox(id).await;
        }
    }

    /// Проверить ящики собеседников: забрать, расшифровать, подтвердить. Возвращает число новых сообщений.
    pub async fn poll_mailboxes(self: &Arc<Self>) -> usize {
        let me = self.identity.node_id;
        let mut peers: Vec<NodeId> = self.watch.lock().await.iter().copied().collect();
        if let Some(h) = &self.history {
            for p in h.peers().await {
                if let Ok(b) = hex::decode(&p) {
                    if let Ok(arr) = <[u8; 32]>::try_from(b.as_slice()) {
                        if !peers.contains(&NodeId(arr)) { peers.push(NodeId(arr)); }
                    }
                }
            }
        }
        let mut got = 0;
        for p in peers {
            let Some(rec) = self.dht.get(&mailbox::key(&me, &p)).await else { continue };
            // ящик пишет только сам отправитель: запись другого публикатора игнорируется
            if rec.publisher != p { warn!("почтовый ящик {}: запись подписана другим узлом, отброшена", p.short()); continue; }
            let Ok(body) = decode::<MailboxBody>(&rec.value) else { continue };
            for env in body.msgs {
                match self.accept_mailbox(p, env).await {
                    Ok(true) => got += 1,
                    Ok(false) => {}
                    Err(e) => warn!("почтовый ящик {}: {e}", p.short()),
                }
            }
        }
        if got > 0 { info!("почтовые ящики DHT: получено {got} сообщений"); }
        got
    }

    /// Одно письмо из ящика `from`. Ok(true) — новое сообщение, Ok(false) — уже было (повторно подтверждено).
    async fn accept_mailbox(self: &Arc<Self>, from: NodeId, env: Vec<u8>) -> Result<bool> {
        let plain = crate::security::e2e::open(&self.identity, &env)?;
        let msg: AppMessage = decode(&plain).map_err(|e| anyhow::anyhow!("AppMessage decode: {e}"))?;
        if msg.from != from { bail!("отправитель письма не совпадает с владельцем ящика"); }
        if !self.verify_signature(&msg).await { bail!("неверная подпись письма"); }
        if !matches!(msg.kind, MessageKind::Text | MessageKind::GroupText) { bail!("в ящике допускаются только сообщения"); }
        let id_hex = hex::encode(&msg.message_id);
        let dup = match &self.history { Some(h) => h.has_incoming(&id_hex).await, None => false };
        if dup {
            // уже получено (например, APP_ACK потерялся) — подтвердить ещё раз, чтобы отправитель убрал из ящика
            let me = self.clone();
            let (pk, id) = (msg.sender_pubkey.clone(), msg.message_id.clone());
            tokio::spawn(async move { me.send_ack(from, pk, id).await });
            return Ok(false);
        }
        self.seen.lock().await.check_window(&msg.message_id, msg.timestamp_ms, self.cfg.mailbox.max_age_s * 1000)
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        self.remember_key(msg.from, &msg.sender_pubkey).await;
        self.accept_message(msg, true).await;
        Ok(true)
    }

    // ── измерительный поток (план звонков, шаг 0) ──────────────────────────

    /// Поток из `count` пакетов по `size` байт с периодом `interval_ms` — как голос (50 пакетов/с по ~80 байт).
    /// Пакеты уходят по расписанию, не дожидаясь подтверждения предыдущего: так ведёт себя источник звука.
    /// Сначала один «прогревочный» пакет (lookup, туннель, ключ получателя) — его время не входит в поток.
    pub async fn probe_stream(self: &Arc<Self>, target: NodeId, stream: &str, count: u32, interval_ms: u64, size: usize)
        -> Result<serde_json::Value>
    {
        let size = size.min(1200);
        let body = |seq: u32| ProbeBody { stream: stream.to_owned(), seq, sent_us: now_us(), pad: vec![0u8; size] };
        let warm = self.build_kind(target, MessageKind::Probe, &body(u32::MAX))?;
        let t_warm = now_ms();
        self.deliver(target, None, &warm).await?;
        let warm_ms = now_ms().saturating_sub(t_warm);

        let start = tokio::time::Instant::now();
        let mut tasks = Vec::with_capacity(count as usize);
        for seq in 0..count {
            tokio::time::sleep_until(start + std::time::Duration::from_millis(interval_ms * seq as u64)).await;
            let me = self.clone();
            let m = self.build_kind(target, MessageKind::Probe, &body(seq))?;
            tasks.push(tokio::spawn(async move {
                let t = now_us();
                let r = me.deliver(target, None, &m).await;
                (r.is_ok(), now_us().saturating_sub(t), r.err().map(|e| e.to_string()))
            }));
        }
        let mut acked = vec![];
        let mut errors: Vec<String> = vec![];
        for t in tasks {
            if let Ok((ok, us, e)) = t.await {
                if ok { acked.push(us as f64 / 1000.0); } else if errors.len() < 5 { errors.extend(e); }
            }
        }
        let sent_span_ms = start.elapsed().as_secs_f64() * 1000.0;
        Ok(serde_json::json!({
            "stream": stream, "count": count, "interval_ms": interval_ms, "size": size,
            "warmup_ms": warm_ms, "acked": acked.len(), "failed": count as usize - acked.len(),
            "ack_ms": acked, "span_ms": sent_span_ms, "errors": errors,
        }))
    }

    /// Принятые пакеты потока `stream`.
    pub async fn probe_results(&self, stream: &str) -> Vec<ProbeRx> {
        self.probes.lock().await.iter().filter(|p| p.stream == stream && p.seq != u32::MAX).cloned().collect()
    }

    // ── «печатает», прочтение, группы ──────────────────────────────────────

    /// «Печатает…»: только по уже установленному туннелю (ради индикатора туннель не строится).
    /// Ok(false) — туннеля нет, ничего не отправлено.
    pub async fn send_typing(&self, target: NodeId) -> Result<bool> {
        if !(self.tunnels.is_usable(&target).await && self.known_key(&target).await.is_some()) { return Ok(false); }
        let m = self.build_kind(target, MessageKind::Typing, &())?;
        self.deliver(target, None, &m).await?;
        Ok(true)
    }

    /// Отчёт о прочтении сообщений `ids` (hex) собеседника `target`.
    pub async fn send_read(&self, target: NodeId, ids: &[String]) -> Result<()> {
        let ids: Vec<Vec<u8>> = ids.iter().filter_map(|i| hex::decode(i).ok()).filter(|b| b.len() == 16).take(200).collect();
        if ids.is_empty() { return Ok(()); }
        let m = self.build_kind(target, MessageKind::Read, &ReadBody { ids })?;
        self.deliver(target, None, &m).await?;
        Ok(())
    }

    /// Кто сейчас печатает: NodeID → до какого момента (unix ms).
    pub async fn typing_now(&self) -> HashMap<String, u64> {
        let now = now_ms();
        let mut t = self.typing.lock().await;
        t.retain(|_, until| *until > now);
        t.iter().map(|(k, v)| (k.to_hex(), *v)).collect()
    }

    /// Групповое сообщение: копия каждому участнику (своё шифрование), ожидание APP_ACK от всех.
    /// В истории — одна запись с собеседником `g:<group_id>`.
    pub async fn send_group(self: &Arc<Self>, group_id: &str, name: &str, members: &[NodeId], text: &str,
                            wait: std::time::Duration) -> Result<Vec<GroupResult>> {
        if group_id.is_empty() || group_id.len() > 64 || !group_id.bytes().all(|b| b.is_ascii_hexdigit()) {
            bail!("group_id: до 64 hex-символов");
        }
        if text.len() > crate::tunnel::forwarder::MAX_SEGMENT_SIZE - 4096 { bail!("сообщение слишком длинное"); }
        let me = self.identity.node_id;
        let mut all: Vec<String> = members.iter().map(|m| m.to_hex()).collect();
        if !all.contains(&me.to_hex()) { all.push(me.to_hex()); }
        let body = GroupBody { group_id: group_id.into(), name: name.into(), members: all, text: text.into() };
        let t0 = now_ms();
        let tasks = members.iter().copied().filter(|m| *m != me).map(|m| {
            let (me2, body) = (self.clone(), body.clone());
            async move {
                let msg = match me2.build_kind(m, MessageKind::GroupText, &body) {
                    Ok(x) => x, Err(e) => return GroupResult { member: m.to_hex(), status: "failed".into(), error: Some(e.to_string()) },
                };
                let rx = me2.register_ack_wait(msg.message_id.clone(), m).await;
                let res = match me2.deliver(m, None, &msg).await {
                    Ok(_) => tokio::time::timeout(wait, rx).await.map_err(|_| "нет подтверждения доставки".to_string())
                        .and_then(|r| r.map_err(|_| "ожидание прервано".to_string())),
                    Err(e) => Err(e.to_string()),
                };
                match res {
                    Ok(()) => GroupResult { member: m.to_hex(), status: "delivered".into(), error: None },
                    Err(e) => {
                        me2.waiting.lock().await.remove(&msg.message_id);
                        if me2.cfg.mailbox.enabled && me2.queue_offline(m, &msg).await.is_ok() {
                            GroupResult { member: m.to_hex(), status: "queued".into(), error: Some(e) }
                        } else {
                            GroupResult { member: m.to_hex(), status: "failed".into(), error: Some(e) }
                        }
                    }
                }
            }
        });
        let results: Vec<GroupResult> = futures::future::join_all(tasks).await;
        let ok = results.iter().filter(|r| r.status == "delivered").count();
        let status = if results.is_empty() { "failed" } else if ok == results.len() { "delivered" }
                     else if results.iter().all(|r| r.status != "failed") { "queued" } else { "partial" };
        if let Some(h) = &self.history {
            let errs: Vec<String> = results.iter().filter(|r| r.status != "delivered")
                .map(|r| format!("{}: {}", &r.member[..8], r.error.clone().unwrap_or_default())).collect();
            h.append(&HistoryEntry {
                ts_ms: now_ms(), dir: "out".into(), peer: format!("g:{group_id}"),
                message_id: hex::encode(uuid::Uuid::new_v4().as_bytes()), status: status.into(), text: Some(text.into()),
                rtt_ms: Some(now_ms().saturating_sub(t0)), error: (!errs.is_empty()).then(|| errs.join("; ")),
                lookup_ms: None, tunnel_build_ms: None, tunnel_send_ms: None,
                group: Some(group_id.into()), group_name: Some(name.into()), members: Some(body.members.clone()), read_ms: None,
            }).await;
        }
        self.events.emit("status", serde_json::json!({ "peer": format!("g:{group_id}") })).await;
        Ok(results)
    }

    /// Удалить переписку с собеседником (или группу `g:<id>`).
    pub async fn delete_history(&self, peer: &str) -> Result<usize> {
        let Some(h) = &self.history else { return Ok(0) };
        let n = h.delete_peer(peer).await?;
        self.events.emit("history", serde_json::json!({ "peer": peer })).await;
        Ok(n)
    }

    /// Принятое сообщение (Text / GroupText): история, inbox, событие, APP_ACK.
    async fn accept_message(self: &Arc<Self>, msg: AppMessage, via_mailbox: bool) {
        let (from, pk, id) = (msg.from, msg.sender_pubkey.clone(), msg.message_id.clone());
        let group = msg.group();
        if let Some(g) = &group {
            if let Some(h) = &self.history {
                h.append(&HistoryEntry {
                    ts_ms: now_ms(), dir: "in".into(), peer: from.to_hex(), message_id: hex::encode(&id),
                    status: "received".into(), text: Some(g.text.clone()), rtt_ms: None, error: None,
                    lookup_ms: None, tunnel_build_ms: None, tunnel_send_ms: None,
                    group: Some(g.group_id.clone()), group_name: Some(g.name.clone()), members: Some(g.members.clone()), read_ms: None,
                }).await;
            }
        } else {
            self.log("in", &from, &msg, "received", None, None).await;
        }
        self.typing.lock().await.remove(&from);
        self.events.emit("message", serde_json::json!({
            "peer": from.to_hex(), "message_id": hex::encode(&id), "via_mailbox": via_mailbox,
            "text": group.as_ref().map(|g| g.text.clone()).or_else(|| msg.text().map(str::to_owned)),
            "group": group.as_ref().map(|g| g.group_id.clone()), "group_name": group.as_ref().map(|g| g.name.clone()),
            "members": group.as_ref().map(|g| g.members.clone()),
        })).await;
        {
            let mut inbox = self.inbox.lock().await;
            if inbox.len() >= 100 { inbox.pop_front(); }
            inbox.push_back(msg);
        }
        let me = self.clone();
        tokio::spawn(async move { me.send_ack(from, pk, id).await });
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
                    None => {
                        drop(w);
                        // подтверждение письма из почтового ящика DHT: убрать из очереди, обновить ящик
                        let id = hex::encode(&msg.payload);
                        if self.outbox.lock().await.remove(&msg.from, &id) {
                            info!("письмо {} из почтового ящика получено {}", &id[..8], msg.from.short());
                            if let Some(h) = &self.history {
                                h.append(&HistoryEntry::status_update(&msg.from.to_hex(), &id, Some("delivered"), None)).await;
                            }
                            self.events.emit("status", serde_json::json!({ "peer": msg.from.to_hex(), "message_id": id, "status": "delivered" })).await;
                            let me = self.clone();
                            let to = msg.from;
                            tokio::spawn(async move { me.publish_mailbox(to).await });
                        } else {
                            tracing::debug!("APP_ACK на неизвестное сообщение");
                        }
                    }
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
            MessageKind::Text => self.accept_message(msg, false).await,
            MessageKind::GroupText => {
                // отправитель должен быть в составе группы, который сам прислал
                let Some(g) = msg.group() else { bail!("некорректное групповое сообщение") };
                if !g.members.contains(&msg.from.to_hex()) { bail!("отправитель не входит в группу"); }
                self.accept_message(msg, false).await;
            }
            MessageKind::CallOffer | MessageKind::CallAnswer | MessageKind::CallReject | MessageKind::CallHangup
            | MessageKind::CallReport => {
                self.on_call_message(msg).await?;
            }
            MessageKind::Probe => {
                let recv_us = now_us();
                let Ok(b) = decode::<ProbeBody>(&msg.payload) else { bail!("некорректный измерительный пакет") };
                let mut p = self.probes.lock().await;
                if p.len() >= 50_000 { p.pop_front(); }
                p.push_back(ProbeRx { stream: b.stream, seq: b.seq, sent_us: b.sent_us, recv_us, from: msg.from.to_hex() });
            }
            MessageKind::Typing => {
                let until = now_ms() + 6000;
                self.typing.lock().await.insert(msg.from, until);
                self.events.emit("typing", serde_json::json!({ "peer": msg.from.to_hex(), "until_ms": until })).await;
            }
            MessageKind::Read => {
                let Ok(body) = decode::<ReadBody>(&msg.payload) else { bail!("некорректный отчёт о прочтении") };
                let ids: Vec<String> = body.ids.iter().take(200).map(hex::encode).collect();
                if let Some(h) = &self.history {
                    // строка-обновление применяется только к нашим сообщениям этому же собеседнику
                    for id in &ids { h.append(&HistoryEntry::status_update(&msg.from.to_hex(), id, None, Some(msg.timestamp_ms))).await; }
                }
                self.events.emit("read", serde_json::json!({ "peer": msg.from.to_hex(), "ids": ids })).await;
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
        self.events.emit("file", serde_json::json!({ "transfer_id": t.transfer_id, "status": t.status, "peer": t.peer })).await;
        let mut v = self.transfers.lock().await;
        match v.iter_mut().find(|x| x.transfer_id == t.transfer_id) {
            Some(x) => *x = t,
            None => { if v.len() >= 50 { v.pop_front(); } v.push_back(t); }
        }
    }

    async fn update_transfer(&self, id: &str, f: impl FnOnce(&mut TransferInfo)) {
        let st = {
            let mut v = self.transfers.lock().await;
            v.iter_mut().find(|x| x.transfer_id == id).map(|x| { f(x); (x.status.clone(), x.peer.clone(), x.done_blocks) })
        };
        // о прогрессе блоков не сообщаем каждый раз — только смена статуса и каждые 8 блоков
        if let Some((status, peer, done)) = st {
            if status != "sending" && status != "receiving" || done % 8 == 0 {
                self.events.emit("file", serde_json::json!({ "transfer_id": id, "status": status, "peer": peer })).await;
            }
        }
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
                            rtt_ms: None, error: err.clone(), lookup_ms: None, tunnel_build_ms: None, tunnel_send_ms: None, group: None, group_name: None, members: None, read_ms: None }).await;
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
                lookup_ms: None, tunnel_build_ms: None, tunnel_send_ms: None, group: None, group_name: None, members: None, read_ms: None }).await;
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
