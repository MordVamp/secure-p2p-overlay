//! app.rs — состояние и интерфейс приложения.
//!
//! Сетевые вызовы не блокируют интерфейс: опрос статуса/истории идёт в отдельном
//! потоке, действия пользователя — в короткоживущих потоках; результаты приходят
//! сообщениями `Msg` через канал.

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use eframe::egui::{self, Align, Color32, CornerRadius, Layout, Margin, RichText, ScrollArea, Stroke, Vec2};
use serde_json::Value;

use crate::api::{self, is_node_id, short};
use crate::node_proc::{find_node_binary, LocalNode};
use crate::settings::{Contact, Mode, Settings};
use crate::theme::{self, *};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab { Chats, Contacts, Files, Calls, Network, Journal, Connection }

const TABS: [(Tab, &str, &str); 7] = [
    (Tab::Chats, "💬", "Чаты"),
    (Tab::Contacts, "👥", "Контакты и профиль"),
    (Tab::Files, "🗀", "Файлы"),
    (Tab::Calls, "📞", "Звонки"),
    (Tab::Network, "🌐", "Сеть: таблица, туннели, DHT"),
    (Tab::Journal, "🕘", "Журнал отправки и приёма"),
    (Tab::Connection, "⚙", "Подключение к узлу"),
];

#[derive(Clone)]
enum Purpose { OpenChat, AddContact(String) }

enum Msg {
    Status(Result<Value, String>),
    History(Vec<Value>),
    Sent { pending: u64, res: Result<Value, String> },
    Tool { title: String, res: Result<Value, String> },
    Route { peer: String, res: Result<Value, String> },
    Resolved { purpose: Purpose, alias: String, res: Result<Value, String> },
    Alias { name: String, res: Result<Value, String> },
    Files(Value),
    Relays(Value),
    FileSent { name: String, res: Result<Value, String> },
    Safety { peer: String, res: Result<Value, String> },
    Deleted { peer: String, res: Result<Value, String> },
    /// `/api/calls`: сеансы звонков (опрос 0,4 с).
    Calls(Value),
    CallDone { what: String, res: Result<Value, String> },
}

struct ChatMsg {
    /// message_id (hex) — для отчётов о прочтении.
    id: String,
    /// Автор (в группе — кто написал).
    author: String,
    /// Получатель прочитал (отчёт о прочтении).
    read: bool,
    mine: bool,
    text: String,
    ts: u64,
    status: String,
    rtt: Option<u64>,
    lookup: Option<u64>,
    build: Option<u64>,
    send: Option<u64>,
    error: Option<String>,
}

struct Chat { peer: String, msgs: Vec<ChatMsg>, last_ts: u64, incoming: usize }

struct Pending { id: u64, peer: String, text: String, ts: u64 }

struct Toast { text: String, color: Color32, at: Instant }

pub struct App {
    s: Settings,
    tab: Tab,
    port: Arc<AtomicU16>,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    ctx: egui::Context,
    tz_offset_s: i64,

    status: Option<Value>,
    conn_err: Option<String>,
    history: Vec<Value>,
    history_loaded: bool,
    chats: Vec<Chat>,
    seen: HashMap<String, usize>,
    active: Option<String>,
    pending: Vec<Pending>,
    next_pending: u64,
    composer: String,
    new_chat: String,

    local: Option<LocalNode>,
    /// Вспомогательные ретрансляторы (Settings::helpers), запускаются, когда основной узел ответил.
    helpers: Vec<LocalNode>,
    helpers_pending: bool,
    /// QR-код ссылки-приглашения: (ссылка, матрица модулей).
    qr: Option<(String, usize, Vec<bool>)>,
    node_log: Vec<String>,
    log_at: Instant,
    toasts: Vec<Toast>,

    t_ping: String,
    t_lookup: String,
    t_route: String,
    t_key: String,
    t_val: String,
    tool_out: Option<(String, Result<Value, String>)>,
    route: Option<(String, Result<Value, String>)>,
    c_name: String,
    c_id: String,
    alias_in: String,
    j_filter: usize,
    files: Value,
    relays: Value,
    f_target: String,
    f_path: String,
    f_sending: Vec<String>,
    /// Группы из истории: `g:<id>` → (название, участники).
    groups: HashMap<String, (String, Vec<String>)>,
    /// Входящие, о прочтении которых уже сообщено (и вся история на момент запуска).
    reported: std::collections::HashSet<String>,
    typing_sent: HashMap<String, Instant>,
    safety: Option<(String, Result<Value, String>)>,
    confirm_delete: Option<String>,
    /// Звонки: сеансы от узла, звук текущего звонка, для какого сеанса он запущен.
    calls: Value,
    call_audio: Option<crate::voice::CallAudio>,
    call_sid: Option<String>,
    call_err: Option<String>,
    call_attention: Option<String>,
}

fn now_ms() -> u64 { SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0) }

/// Смещение локального часового пояса (без внешних крейтов: `date +%z`).
fn tz_offset_s() -> i64 {
    let out = std::process::Command::new("date").arg("+%z").output().ok();
    let s = out.map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned()).unwrap_or_default();
    if s.len() != 5 { return 0; }
    let sign = if s.starts_with('-') { -1 } else { 1 };
    let h: i64 = s[1..3].parse().unwrap_or(0);
    let m: i64 = s[3..5].parse().unwrap_or(0);
    sign * (h * 3600 + m * 60)
}

fn hex_bytes(h: &str) -> Vec<u8> {
    (0..h.len() / 2).filter_map(|i| u8::from_str_radix(&h[2 * i..2 * i + 2], 16).ok()).collect()
}

/// «Вызов…», «Соединение…» или длительность разговора.
fn call_status(s: &Value) -> String {
    match (s["state"].as_str(), s["duration_s"].as_u64()) {
        (Some("ringing"), _) => "вызов…".into(),
        (Some("ended"), _) => "завершён".into(),
        (_, Some(d)) => format!("{}:{:02}", d / 60, d % 60),
        _ => "соединение…".into(),
    }
}

fn ms(v: Option<u64>) -> String { v.map(|x| format!("{x} мс")).unwrap_or_else(|| "—".into()) }

fn ago(ms_ago: u64) -> String {
    let s = ms_ago / 1000;
    if s < 60 { format!("{s} с") } else if s < 3600 { format!("{} мин", s / 60) } else { format!("{} ч", s / 3600) }
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        theme::apply(&cc.egui_ctx);
        theme::set(&cc.egui_ctx, &Settings::load().theme);
        let (tx, rx) = channel();
        let s = Settings::load();
        let port = Arc::new(AtomicU16::new(0));
        spawn_poller(port.clone(), tx.clone(), cc.egui_ctx.clone());
        let first_run = !std::path::Path::new(&s.state_dir).join("identity.key").exists() && s.mode == Mode::Launch;
        let mut app = Self {
            tab: if first_run { Tab::Connection } else { Tab::Chats },
            s, port, tx, rx, ctx: cc.egui_ctx.clone(), tz_offset_s: tz_offset_s(),
            status: None, conn_err: None, history: vec![], history_loaded: false, chats: vec![],
            seen: HashMap::new(), active: None, pending: vec![], next_pending: 1,
            composer: String::new(), new_chat: String::new(),
            local: None, helpers: vec![], helpers_pending: false, qr: None, node_log: vec![], log_at: Instant::now(), toasts: vec![],
            t_ping: String::new(), t_lookup: String::new(), t_route: String::new(),
            t_key: String::new(), t_val: String::new(), tool_out: None, route: None,
            c_name: String::new(), c_id: String::new(), alias_in: String::new(), j_filter: 0,
            files: Value::Null, relays: Value::Null, f_target: String::new(), f_path: String::new(), f_sending: vec![],
            groups: HashMap::new(), reported: Default::default(), typing_sent: HashMap::new(), safety: None, confirm_delete: None,
            calls: Value::Null, call_audio: None, call_sid: None, call_err: None, call_attention: None,
        };
        if app.s.mode == Mode::Attach || app.s.autostart { app.connect(); }
        app
    }

    // ── фон ────────────────────────────────────────────────────────────────

    fn bg(&self, f: impl FnOnce(u16) -> Msg + Send + 'static) {
        let (tx, ctx, p) = (self.tx.clone(), self.ctx.clone(), self.port.load(Ordering::Relaxed));
        std::thread::spawn(move || { let _ = tx.send(f(p)); ctx.request_repaint(); });
    }

    fn toast(&mut self, text: impl Into<String>, color: Color32) {
        self.toasts.push(Toast { text: text.into(), color, at: Instant::now() });
    }

    fn connected(&self) -> bool { self.status.is_some() && self.conn_err.is_none() }

    fn my_id(&self) -> String { self.status.as_ref().map(|s| api::s(s, "node_id")).unwrap_or_default() }

    fn feature(&self, name: &str) -> bool {
        self.status.as_ref().and_then(|s| s["features"][name].as_bool()).unwrap_or(false)
    }

    fn display_name(&self, id: &str) -> String {
        if let Some((name, _)) = self.groups.get(id) { return format!("👥 {name}"); }
        if id == self.my_id() { return "Вы".into(); }
        self.s.contact_name(id).map(str::to_owned).unwrap_or_else(|| format!("{}…", short(id)))
    }

    fn is_group(id: &str) -> bool { id.starts_with("g:") }

    /// Собеседник печатает (по данным узла: `/api/status` → `typing`).
    fn typing(&self, peer: &str) -> bool {
        let now = now_ms();
        self.status.as_ref().and_then(|s| s["typing"][peer].as_u64()).map(|t| t > now).unwrap_or(false)
    }

    fn connect(&mut self) {
        self.disconnect();
        match self.s.mode {
            // на порту API уже отвечает узел (напр. остался от прошлого запуска) — подключаемся к нему
            Mode::Launch if api::get(self.s.gui_port, "/api/status", &[]).is_ok() => {
                self.toast(format!("На порту {} уже работает узел — подключаюсь к нему", self.s.gui_port), YELLOW());
                self.port.store(self.s.gui_port, Ordering::Relaxed);
            }
            Mode::Launch => match LocalNode::start(&self.s) {
                Ok(n) => {
                    self.toast(format!("Узел запущен (pid {})", n.pid()), GREEN());
                    self.local = Some(n);
                    self.helpers_pending = self.s.helpers > 0;
                    self.port.store(self.s.gui_port, Ordering::Relaxed);
                }
                Err(e) => self.toast(e, RED()),
            },
            Mode::Attach => self.port.store(self.s.attach_port, Ordering::Relaxed),
        }
        let _ = self.s.save();
    }

    fn disconnect(&mut self) {
        self.port.store(0, Ordering::Relaxed);
        if let Some(mut n) = self.local.take() { n.stop(); }
        self.helpers.clear(); // Drop останавливает процессы
        self.helpers_pending = false;
        self.status = None;
        self.conn_err = None;
        self.history.clear();
        self.history_loaded = false;
        self.chats.clear();
    }

    fn drain(&mut self) {
        while let Ok(m) = self.rx.try_recv() {
            match m {
                Msg::Status(Ok(v)) => {
                    // впервые на связи: контакты — узлу (знакомые: их файлы до 16 МиБ принимаются без вопроса)
                    if self.status.is_none() { self.watch_contacts(); }
                    self.status = Some(v);
                    self.conn_err = None;
                }
                Msg::Status(Err(e)) => { if self.port.load(Ordering::Relaxed) != 0 { self.conn_err = Some(e); } }
                Msg::History(h) => { if self.port.load(Ordering::Relaxed) != 0 { self.history = h; self.rebuild_chats(); } }
                Msg::Sent { pending, res } => {
                    self.pending.retain(|p| p.id != pending);
                    match res {
                        Err(e) => self.toast(format!("Не доставлено: {e}"), RED()),
                        Ok(v) if v["queued"].as_bool() == Some(true) =>
                            self.toast("Получатель недоступен — сообщение в почтовом ящике DHT, он заберёт его сам", YELLOW()),
                        Ok(v) if v["results"].is_array() => {
                            let bad = v["results"].as_array().unwrap().iter().filter(|r| r["status"] != "delivered").count();
                            if bad > 0 { self.toast(format!("Группа: не доставлено сразу {bad} участникам"), YELLOW()); }
                        }
                        _ => {}
                    }
                }
                Msg::Tool { title, res } => self.tool_out = Some((title, res)),
                Msg::Route { peer, res } => self.route = Some((peer, res)),
                Msg::Resolved { purpose, alias, res } => match api::ok(res) {
                    Ok(v) => {
                        let id = api::s(&v, "node_id");
                        match purpose {
                            Purpose::OpenChat => {
                                if self.s.contact_name(&id).is_none() {
                                    self.s.contacts.push(Contact { name: format!("@{alias}"), node_id: id.clone() });
                                    let _ = self.s.save();
                                }
                                self.open_chat(id);
                            }
                            Purpose::AddContact(name) => self.add_contact(name, id),
                        }
                    }
                    Err(e) => self.toast(format!("@{alias}: {e}"), RED()),
                },
                Msg::Files(v) => self.files = v,
                Msg::Relays(v) => if !v.is_null() { self.relays = v },
                Msg::FileSent { name, res } => {
                    self.f_sending.retain(|n| *n != name);
                    match api::ok(res) {
                        Ok(v) => {
                            let r = &v["report"];
                            self.toast(format!("«{name}» доставлен: {:.2} МиБ/с, служебная доля {:.1}%",
                                               r["goodput_mib_s"].as_f64().unwrap_or(0.0), r["overhead_share"].as_f64().unwrap_or(0.0) * 100.0), GREEN());
                        }
                        Err(e) => self.toast(format!("«{name}» не доставлен: {e}"), RED()),
                    }
                }
                Msg::Safety { peer, res } => self.safety = Some((peer, res)),
                Msg::Deleted { peer, res } => match api::ok(res) {
                    Ok(v) => {
                        self.toast(format!("Переписка удалена ({} записей)", api::s(&v, "removed")), GREEN());
                        if self.active.as_deref() == Some(peer.as_str()) { self.active = None; }
                    }
                    Err(e) => self.toast(e, RED()),
                },
                Msg::Calls(v) => { self.calls = v; self.sync_call_audio(); }
                Msg::CallDone { what, res } => if let Err(e) = api::ok(res) { self.toast(format!("{what}: {e}"), RED()); },
                Msg::Alias { name, res } => match api::ok(res) {
                    Ok(v) => {
                        self.toast(format!("Псевдоним @{name} опубликован ({} реплик)", api::s(&v, "stored_on")), GREEN());
                        self.s.my_alias = name;
                        let _ = self.s.save();
                    }
                    Err(e) => self.toast(format!("@{name}: {e}"), RED()),
                },
            }
        }
    }

    fn rebuild_chats(&mut self) {
        let mut map: BTreeMap<String, Chat> = BTreeMap::new();
        let first_load = !self.history_loaded;
        for e in &self.history {
            let author = api::s(e, "peer");
            if author.is_empty() { continue; }
            let group = e["group"].as_str().map(str::to_owned);
            let peer = match &group { Some(g) => format!("g:{g}"), None => author.clone() };
            if let Some(g) = &group {
                let members: Vec<String> = e["members"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_owned)).collect())
                    .unwrap_or_default();
                let name = e["group_name"].as_str().unwrap_or("Группа").to_owned();
                if !members.is_empty() { self.groups.insert(format!("g:{g}"), (name, members)); }
            }
            let mine = api::s(e, "dir") == "out";
            let ts = api::u(e, "ts_ms").unwrap_or(0);
            let id = api::s(e, "message_id");
            // вся история на момент запуска считается уже прочитанной — отчёты только о новом
            if first_load && !mine { self.reported.insert(id.clone()); }
            let c = map.entry(peer.clone()).or_insert(Chat { peer, msgs: vec![], last_ts: 0, incoming: 0 });
            c.last_ts = c.last_ts.max(ts);
            if !mine { c.incoming += 1; }
            c.msgs.push(ChatMsg {
                id, author, read: api::u(e, "read_ms").is_some(),
                mine, ts,
                text: api::s(e, "text"),
                status: api::s(e, "status"),
                rtt: api::u(e, "rtt_ms"),
                lookup: api::u(e, "lookup_ms"),
                build: api::u(e, "tunnel_build_ms"),
                send: api::u(e, "tunnel_send_ms"),
                error: e["error"].as_str().map(str::to_owned),
            });
        }
        let mut chats: Vec<Chat> = map.into_values().collect();
        chats.sort_by(|a, b| b.last_ts.cmp(&a.last_ts));
        let mut fresh = vec![];
        for c in &chats {
            let seen = self.seen.entry(c.peer.clone()).or_insert(if self.history_loaded { 0 } else { c.incoming });
            let watching = self.tab == Tab::Chats && self.active.as_deref() == Some(c.peer.as_str());
            if watching { *seen = c.incoming; }
            if c.incoming > *seen && self.history_loaded && !watching {
                if let Some(m) = c.msgs.iter().rev().find(|m| !m.mine) { fresh.push((c.peer.clone(), m.text.clone())); }
            }
        }
        self.chats = chats;
        for (peer, text) in fresh {
            let notified = self.toasts.iter().any(|t| t.text.ends_with(&text));
            if !notified { self.toast(format!("✉ {}: {text}", self.display_name(&peer)), ACCENT()); }
        }
        self.history_loaded = true;
    }

    fn unread(&self, peer: &str) -> usize {
        let inc = self.chats.iter().find(|c| c.peer == peer).map(|c| c.incoming).unwrap_or(0);
        inc.saturating_sub(*self.seen.get(peer).unwrap_or(&inc))
    }

    fn open_chat(&mut self, id: String) {
        if let Some(c) = self.chats.iter().find(|c| c.peer == id) { self.seen.insert(id.clone(), c.incoming); }
        self.active = Some(id);
        self.tab = Tab::Chats;
        self.route = None;
    }

    fn watch_contacts(&self) {
        let ids: Vec<String> = self.s.contacts.iter().map(|c| c.node_id.clone()).collect();
        if ids.is_empty() { return; }
        self.bg(move |p| { let _ = api::post(p, "/api/watch", &[("peers", &ids.join(","))]); Msg::Relays(Value::Null) });
    }

    fn add_contact(&mut self, name: String, id: String) {
        let name = if name.trim().is_empty() { format!("{}…", short(&id)) } else { name.trim().to_owned() };
        match self.s.contacts.iter_mut().find(|c| c.node_id == id) {
            Some(c) => c.name = name.clone(),
            None => self.s.contacts.push(Contact { name: name.clone(), node_id: id }),
        }
        let _ = self.s.save();
        self.watch_contacts();
        self.toast(format!("Контакт «{name}» сохранён"), GREEN());
    }

    /// NodeID или @псевдоним → действие (псевдоним разрешается через DHT в фоне).
    fn with_target(&mut self, input: &str, purpose: Purpose) {
        let t = input.trim();
        if is_node_id(t) {
            match purpose {
                Purpose::OpenChat => self.open_chat(t.to_lowercase()),
                Purpose::AddContact(name) => self.add_contact(name, t.to_lowercase()),
            }
        } else if let Some(alias) = t.strip_prefix('@').map(str::to_owned).filter(|a| !a.is_empty()) {
            self.bg(move |p| {
                let res = api::get(p, "/api/resolve", &[("alias", &alias)]);
                Msg::Resolved { purpose, alias, res }
            });
        } else {
            self.toast("Нужен NodeID (64 hex) или @псевдоним", YELLOW());
        }
    }

    fn send(&mut self) {
        let (Some(peer), text) = (self.active.clone(), self.composer.trim().to_owned()) else { return };
        if text.is_empty() { return; }
        self.composer.clear();
        self.typing_sent.remove(&peer);
        let id = self.next_pending;
        self.next_pending += 1;
        self.pending.push(Pending { id, peer: peer.clone(), text: text.clone(), ts: now_ms() });
        let tx = self.tx.clone();
        let me = self.my_id();
        let group = self.groups.get(&peer).cloned();
        self.bg(move |p| {
            let res = match group {
                Some((name, members)) => {
                    let m: Vec<String> = members.into_iter().filter(|x| *x != me).collect();
                    let gid = peer.trim_start_matches("g:").to_owned();
                    api::ok(api::post(p, "/api/send_group", &[("group_id", &gid), ("name", &name),
                                                             ("members", &m.join(",")), ("text", &text)]))
                }
                None => api::ok(api::post(p, "/api/send", &[("target", &peer), ("text", &text)])),
            };
            if let Ok(h) = api::get(p, "/api/history", &[("limit", "1000")]) {
                let _ = tx.send(Msg::History(h["entries"].as_array().cloned().unwrap_or_default()));
            }
            Msg::Sent { pending: id, res }
        });
    }

    /// Отчёт о прочтении новых входящих в открытой беседе (если включено в настройках).
    fn report_read(&mut self, peer: &str) {
        if !self.s.send_receipts || Self::is_group(peer) { return; }
        let Some(c) = self.chats.iter().find(|c| c.peer == peer) else { return };
        let ids: Vec<String> = c.msgs.iter().filter(|m| !m.mine && m.id.len() == 32 && m.status == "received" && !self.reported.contains(&m.id))
            .map(|m| m.id.clone()).collect();
        if ids.is_empty() { return; }
        self.reported.extend(ids.iter().cloned());
        let peer = peer.to_owned();
        self.bg(move |p| { let _ = api::post(p, "/api/read", &[("target", &peer), ("ids", &ids.join(","))]); Msg::Relays(Value::Null) });
    }

    /// «Печатает…»: не чаще раза в 4 с, только в личных беседах.
    fn report_typing(&mut self, peer: &str) {
        if !self.s.send_typing || Self::is_group(peer) { return; }
        if self.typing_sent.get(peer).is_some_and(|t| t.elapsed() < Duration::from_secs(4)) { return; }
        self.typing_sent.insert(peer.to_owned(), Instant::now());
        let peer = peer.to_owned();
        self.bg(move |p| { let _ = api::post(p, "/api/typing", &[("target", &peer)]); Msg::Relays(Value::Null) });
    }

    fn request_route(&self, peer: String) {
        self.bg(move |p| { let res = api::get(p, "/api/route", &[("target", &peer)]); Msg::Route { peer, res } });
    }

    fn time(&self, ts_ms: u64) -> String {
        let t = (ts_ms / 1000) as i64 + self.tz_offset_s;
        let d = t.rem_euclid(86_400);
        format!("{:02}:{:02}", d / 3600, (d % 3600) / 60)
    }
}

fn spawn_poller(port: Arc<AtomicU16>, tx: Sender<Msg>, ctx: egui::Context) {
    // звонки — отдельно и чаще: входящий вызов, длительность, кто говорит
    {
        let (port, tx, ctx) = (port.clone(), tx.clone(), ctx.clone());
        std::thread::spawn(move || loop {
            let p = port.load(Ordering::Relaxed);
            if p != 0 {
                if let Ok(v) = api::call(p, "GET", "/api/calls", &[], Duration::from_secs(3)) {
                    if tx.send(Msg::Calls(v)).is_err() { break; }
                    ctx.request_repaint();
                }
            }
            std::thread::sleep(Duration::from_millis(400));
        });
    }
    std::thread::spawn(move || loop {
        let p = port.load(Ordering::Relaxed);
        if p != 0 {
            let st = api::get(p, "/api/status", &[]);
            let ok = st.is_ok();
            if tx.send(Msg::Status(st)).is_err() { break; }
            if ok {
                if let Ok(h) = api::get(p, "/api/history", &[("limit", "1000")]) {
                    let _ = tx.send(Msg::History(h["entries"].as_array().cloned().unwrap_or_default()));
                }
                if let Ok(f) = api::get(p, "/api/files", &[]) { let _ = tx.send(Msg::Files(f)); }
                if let Ok(r) = api::get(p, "/api/relays", &[]) { let _ = tx.send(Msg::Relays(r)); }
            }
            ctx.request_repaint();
        }
        std::thread::sleep(Duration::from_millis(1200));
    });
}

// ── интерфейс ────────────────────────────────────────────────────────────────

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.drain();
        if let Some(n) = &mut self.local {
            if !n.running() {
                self.local = None;
                self.port.store(0, Ordering::Relaxed);
                self.status = None;
                self.toast("Процесс узла завершился — см. журнал на вкладке «Подключение»", RED());
            }
        }
        if self.helpers_pending && self.local.is_some() && self.connected() {
            self.helpers_pending = false;
            for i in 1..=self.s.helpers as u16 {
                match LocalNode::start_helper(&self.s, i) {
                    Ok(n) => self.helpers.push(n),
                    Err(e) => { self.toast(format!("ретранслятор {i}: {e}"), RED()); break; }
                }
            }
            if !self.helpers.is_empty() {
                self.toast(format!("Запущено ретрансляторов: {}", self.helpers.len()), GREEN());
            }
        }
        self.rail(ctx);
        self.call_banner(ctx);
        match self.tab {
            Tab::Chats => self.chats_tab(ctx),
            tab => {
                egui::CentralPanel::default()
                    .frame(egui::Frame::default().fill(BG_MAIN()).inner_margin(Margin::same(24)))
                    .show(ctx, |ui| {
                        theme::decor_bg(ui, true);
                        ScrollArea::vertical().auto_shrink([false; 2]).show(ui, |ui| match tab {
                            Tab::Contacts => self.contacts_tab(ui),
                            Tab::Files => self.files_tab(ui),
                            Tab::Calls => self.calls_tab(ui),
                            Tab::Network => self.network_tab(ui),
                            Tab::Journal => self.journal_tab(ui),
                            Tab::Connection => self.connection_tab(ui),
                            Tab::Chats => {}
                        });
                    });
            }
        }
        theme::decor_top(ctx);
        self.toasts_ui(ctx);
        ctx.request_repaint_after(Duration::from_millis(if theme::animated() { 40 } else { 500 }));
    }

    /// Закрытие окна: остановить узел, запущенный приложением.
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        if let Some(mut n) = self.local.take() { n.stop(); }
        self.helpers.clear();
    }
}

impl App {
    fn rail(&mut self, ctx: &egui::Context) {
        egui::SidePanel::left("rail").resizable(false).exact_width(76.0)
            .frame(egui::Frame::default().fill(BG_RAIL()).inner_margin(Margin::symmetric(14, 16)))
            .show(ctx, |ui| {
                ui.vertical_centered(|ui| {
                    let (r, _) = ui.allocate_exact_size(Vec2::splat(48.0), egui::Sense::hover());
                    ui.painter().rect_filled(r, CornerRadius::same(16), ACCENT());
                    ui.painter().text(r.center(), egui::Align2::CENTER_CENTER, "P2P",
                                      egui::FontId::proportional(15.0), Color32::WHITE);
                    ui.add_space(6.0);
                    ui.separator();
                    ui.add_space(6.0);
                    let unread: usize = self.chats.iter().map(|c| self.unread(&c.peer)).sum();
                    for (tab, icon, tip) in TABS {
                        let badge = if tab == Tab::Chats { unread } else { 0 };
                        let soon = matches!(tab, Tab::Files | Tab::Calls) && !self.feature(if tab == Tab::Files { "files" } else { "calls" });
                        if rail_button(ui, icon, self.tab == tab, badge, soon).on_hover_text(tip).clicked() {
                            self.tab = tab;
                            if tab == Tab::Chats { if let Some(a) = self.active.clone() { self.open_chat(a); } }
                        }
                        ui.add_space(4.0);
                    }
                    ui.with_layout(Layout::bottom_up(Align::Center), |ui| {
                        let (c, tip) = if self.connected() { (GREEN(), "узел на связи".to_owned()) }
                            else if self.port.load(Ordering::Relaxed) != 0 { (YELLOW(), format!("подключение… {}", self.conn_err.clone().unwrap_or_default())) }
                            else { (RED(), "не подключено".to_owned()) };
                        let (r, resp) = ui.allocate_exact_size(Vec2::splat(18.0), egui::Sense::hover());
                        ui.painter().circle_filled(r.center(), 6.0, c);
                        resp.on_hover_text(tip);
                    });
                });
            });
    }

    fn not_connected_hint(&mut self, ui: &mut egui::Ui) {
        ui.add_space(60.0);
        ui.vertical_centered(|ui| {
            ui.label(RichText::new("🔌").size(56.0));
            ui.label(RichText::new("Нет связи с узлом").size(20.0).strong());
            ui.label(RichText::new(self.conn_err.clone().unwrap_or_else(|| "Запустите свой узел или подключитесь к работающему".into())).color(MUTED()));
            ui.add_space(8.0);
            if ui.add(primary("Открыть «Подключение»")).clicked() { self.tab = Tab::Connection; }
        });
    }

    // ── Чаты ───────────────────────────────────────────────────────────────

    fn chats_tab(&mut self, ctx: &egui::Context) {
        egui::SidePanel::left("chat_list").resizable(false).exact_width(290.0)
            .frame(egui::Frame::default().fill(BG_SIDE()).inner_margin(Margin::symmetric(12, 16)))
            .show(ctx, |ui| {
                theme::decor_bg(ui, false);
                ui.horizontal(|ui| {
                    theme::glyph(ui, 22.0);
                    ui.label(RichText::new("Чаты").size(20.0).strong());
                });
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    let w = ui.available_width() - 40.0 - ui.spacing().item_spacing.x;
                    let r = ui.add_sized([w, 32.0], field(&mut self.new_chat).hint_text("NodeID или @псевдоним"));
                    let go = ui.add_sized([40.0, 32.0], primary("+")).on_hover_text("Начать чат").clicked()
                        || (r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)));
                    if go && !self.new_chat.trim().is_empty() {
                        let t = std::mem::take(&mut self.new_chat);
                        self.with_target(&t, Purpose::OpenChat);
                    }
                });
                ui.add_space(8.0);
                ScrollArea::vertical().auto_shrink([false; 2]).show(ui, |ui| {
                    let mut peers: Vec<String> = self.chats.iter().map(|c| c.peer.clone()).collect();
                    for c in &self.s.contacts { if !peers.contains(&c.node_id) { peers.push(c.node_id.clone()); } }
                    if let Some(a) = &self.active { if !peers.contains(a) { peers.insert(0, a.clone()); } }
                    if peers.is_empty() {
                        ui.label(RichText::new("Пока нет переписки. Введите NodeID собеседника или его @псевдоним выше.").color(MUTED()));
                    }
                    for peer in peers {
                        if self.chat_row(ui, &peer) { self.open_chat(peer); }
                    }
                });
            });

        egui::CentralPanel::default().frame(egui::Frame::default().fill(BG_MAIN())).show(ctx, |ui| {
            if !self.connected() { self.not_connected_hint(ui); return; }
            match self.active.clone() {
                None => {
                    ui.add_space(80.0);
                    ui.vertical_centered(|ui| {
                        ui.label(RichText::new("💬").size(64.0));
                        ui.label(RichText::new("Выберите чат или начните новый").size(20.0).strong());
                        ui.label(RichText::new("Сообщения идут через туннель из ≥2 ретрансляторов и шифруются сквозным ключом получателя").color(MUTED()));
                    });
                }
                Some(peer) => self.conversation(ui, peer),
            }
        });
    }

    fn chat_row(&self, ui: &mut egui::Ui, peer: &str) -> bool {
        let selected = self.active.as_deref() == Some(peer);
        let chat = self.chats.iter().find(|c| c.peer == peer);
        let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 58.0), egui::Sense::click());
        let fill = if selected { BG_HOVER() } else if resp.hovered() { BG_CARD() } else { Color32::TRANSPARENT };
        ui.painter().rect_filled(rect, CornerRadius::same(10), fill);
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink2(Vec2::new(10.0, 8.0))).layout(Layout::left_to_right(Align::Center)));
        let name = self.display_name(peer);
        theme::avatar(&mut child, peer, &name, 40.0);
        child.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            ui.horizontal(|ui| {
                ui.label(RichText::new(&name).strong());
                if let Some(c) = chat {
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let n = self.unread(peer);
                        if n > 0 {
                            egui::Frame::default().fill(RED()).corner_radius(CornerRadius::same(9)).inner_margin(Margin::symmetric(6, 0))
                                .show(ui, |ui| ui.label(RichText::new(n.to_string()).size(12.0).color(Color32::WHITE)));
                        }
                        ui.label(RichText::new(self.time(c.last_ts)).size(12.0).color(MUTED()));
                    });
                }
            });
            let preview = chat.and_then(|c| c.msgs.last()).map(|m| {
                if m.mine { format!("Вы: {}", m.text) } else { m.text.clone() }
            }).unwrap_or_else(|| format!("{}…", short(peer)));
            ui.add(egui::Label::new(RichText::new(preview).size(13.0).color(MUTED())).truncate());
        });
        resp.clicked()
    }

    fn conversation(&mut self, ui: &mut egui::Ui, peer: String) {
        let name = self.display_name(&peer);
        egui::TopBottomPanel::top("conv_head")
            .frame(egui::Frame::default().fill(BG_MAIN()).inner_margin(Margin::symmetric(20, 12)).stroke(Stroke::new(1.0_f32, BG_RAIL())))
            .show_inside(ui, |ui| {
                ui.horizontal(|ui| {
                    theme::avatar(ui, &peer, &name, 40.0);
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 0.0;
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(&name).size(17.0).strong());
                            if self.s.verified.contains(&peer) { theme::pill(ui, "сверен", GREEN()); }
                        });
                        if self.typing(&peer) {
                            ui.label(RichText::new("печатает…").size(12.0).color(ACCENT()));
                        } else if let Some((_, members)) = self.groups.get(&peer) {
                            let names: Vec<String> = members.iter().map(|m| self.display_name(m)).collect();
                            ui.label(RichText::new(names.join(", ")).size(11.5).color(MUTED()));
                        } else {
                            ui.label(RichText::new(&peer).size(11.5).color(MUTED()).monospace());
                        }
                    });
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if self.confirm_delete.as_deref() == Some(peer.as_str()) {
                            if ui.add(danger("Удалить переписку?")).clicked() {
                                let pp = peer.clone();
                                self.confirm_delete = None;
                                self.bg(move |p| { let res = api::post(p, "/api/history/delete", &[("peer", &pp)]); Msg::Deleted { peer: pp, res } });
                            }
                            if ui.button("Отмена").clicked() { self.confirm_delete = None; }
                        } else if ui.button("🗑").on_hover_text("Удалить переписку на этом устройстве").clicked() {
                            self.confirm_delete = Some(peer.clone());
                        }
                        if !Self::is_group(&peer) {
                            if ui.button("Номер безопасности").on_hover_text("Сверить ключ собеседника лично").clicked() {
                                let pp = peer.clone();
                                self.bg(move |p| { let res = api::get(p, "/api/safety", &[("peer", &pp)]); Msg::Safety { peer: pp, res } });
                            }
                            if ui.button("Маршрут").on_hover_text("Цепочка ретрансляторов текущего туннеля").clicked() {
                                self.request_route(peer.clone());
                            }
                        }
                        if self.feature("calls") {
                            let live = self.live_call().is_some();
                            if ui.add_enabled(!live, primary("📞")).on_hover_text(if Self::is_group(&peer) { "Групповой звонок" } else { "Позвонить" }).clicked() {
                                self.start_call(peer.clone());
                            }
                        }
                        if ui.button("📋").on_hover_text("Скопировать NodeID").clicked() {
                            ui.ctx().copy_text(peer.clone());
                            self.toast("NodeID скопирован", GREEN());
                        }
                        if self.s.contact_name(&peer).is_none() && ui.button("⭐ В контакты").clicked() {
                            self.c_id = peer.clone();
                            self.tab = Tab::Contacts;
                        }
                    });
                });
                if let Some((rp, r)) = &self.route {
                    if *rp == peer {
                        ui.add_space(6.0);
                        route_chain(ui, r);
                    }
                }
                let mut verify: Option<bool> = None;
                if let Some((sp, r)) = &self.safety {
                    if *sp == peer {
                        ui.add_space(6.0);
                        match r {
                            Ok(v) => {
                                ui.label(RichText::new("Номер безопасности — у собеседника на экране должны быть те же цифры:").size(12.5).color(MUTED()));
                                let groups: Vec<&str> = v["safety"].as_str().unwrap_or("").split(' ').collect();
                                for row in groups.chunks(6) {
                                    ui.label(RichText::new(row.join("  ")).monospace().size(17.0));
                                }
                                let ok = self.s.verified.contains(&peer);
                                ui.horizontal(|ui| {
                                    if ui.button(if ok { "Снять отметку «сверен»" } else { "✔ Совпадает — отметить сверенным" }).clicked() { verify = Some(!ok); }
                                    if ui.button("Скрыть").clicked() { verify = Some(ok); }
                                });
                            }
                            Err(e) => { ui.label(RichText::new(e).color(RED())); }
                        }
                    }
                }
                if let Some(v) = verify {
                    self.s.verified.retain(|x| *x != peer);
                    if v { self.s.verified.push(peer.clone()); }
                    let _ = self.s.save();
                    self.safety = None;
                }
            });

        egui::TopBottomPanel::bottom("composer")
            .frame(egui::Frame::default().fill(BG_MAIN()).inner_margin(Margin::symmetric(20, 14)))
            .show_inside(ui, |ui| {
                ui.horizontal(|ui| {
                    let r = ui.add(field(&mut self.composer)
                        .hint_text(format!("Сообщение для {name}"))
                        .desired_width(ui.available_width() - 118.0 - ui.spacing().item_spacing.x - 24.0)
                        .margin(Margin::symmetric(12, 10)));
                    let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    if r.changed() && !self.composer.trim().is_empty() { self.report_typing(&peer); }
                    if ui.add_sized([118.0, 40.0], primary("Отправить →")).clicked() || enter {
                        self.send();
                        r.request_focus();
                    }
                });
                ui.label(RichText::new("🔒 E2E: X25519 + AES-256-GCM · подпись Ed25519 · доставка подтверждается APP_ACK").size(11.5).color(MUTED()));
            });

        egui::CentralPanel::default().frame(egui::Frame::default().fill(BG_MAIN()).inner_margin(Margin::symmetric(20, 10)))
            .show_inside(ui, |ui| {
                theme::decor_bg(ui, true);
                ScrollArea::vertical().stick_to_bottom(true).auto_shrink([false; 2]).show(ui, |ui| {
                    let chat = self.chats.iter().find(|c| c.peer == peer);
                    if chat.is_none() && !self.pending.iter().any(|p| p.peer == peer) {
                        ui.add_space(40.0);
                        ui.vertical_centered(|ui| ui.label(RichText::new("Сообщений пока нет — напишите первым").color(MUTED())));
                    }
                    let group = Self::is_group(&peer);
                    if let Some(c) = chat {
                        for m in &c.msgs { self.bubble(ui, m, group); }
                    }
                    for p in self.pending.iter().filter(|p| p.peer == peer) {
                        let m = ChatMsg { id: String::new(), author: String::new(), read: false,
                                          mine: true, text: p.text.clone(), ts: p.ts, status: "pending".into(),
                                          rtt: None, lookup: None, build: None, send: None, error: None };
                        self.bubble(ui, &m, group);
                    }
                    if self.typing(&peer) { ui.label(RichText::new("печатает…").color(ACCENT()).size(13.0)); }
                });
            });
        if ui.ctx().input(|i| i.focused) { self.report_read(&peer); }
    }

    fn bubble(&self, ui: &mut egui::Ui, m: &ChatMsg, group: bool) {
        let max_w = (ui.available_width() * 0.66).max(240.0);
        let text = if m.text.is_empty() { "—" } else { m.text.as_str() };
        let (icon, icon_color) = match m.status.as_str() {
            _ if m.read => ("✔✔ прочитано", ACCENT()),
            "queued" => ("🕓 в почтовом ящике", YELLOW()),
            "partial" => ("✔ не всем", YELLOW()),
            "delivered" => ("✔✔", GREEN()),
            "sent" => ("✔", TEXT()),
            "pending" => ("⏳", YELLOW()),
            "received" => ("", TEXT()),
            _ => ("✖", RED()),
        };
        let footer = format!("{}  {icon}  {}", self.time(m.ts), m.rtt.map(|r| format!("{r} мс")).unwrap_or_default());
        // ширина пузыря = ширина текста (с переносом по max_w), а не всей колонки
        let body = egui::TextStyle::Body.resolve(ui.style());
        let w = ui.fonts(|f| {
            let t = f.layout(text.to_owned(), body.clone(), TEXT(), max_w).size().x;
            let foot = f.layout_no_wrap(footer.clone(), egui::FontId::proportional(11.0), TEXT()).size().x;
            t.max(foot + 6.0).min(max_w).ceil()
        });
        let (fill, radius) = if m.mine {
            (ACCENT_DIM(), CornerRadius { nw: 14, ne: 14, sw: 14, se: 4 })
        } else {
            (BG_CARD(), CornerRadius { nw: 14, ne: 14, sw: 4, se: 14 })
        };
        ui.horizontal(|ui| {
            if m.mine { ui.add_space((ui.available_width() - w - 24.0).max(0.0)); }
            let resp = egui::Frame::default().fill(fill).corner_radius(radius).inner_margin(Margin::symmetric(12, 8))
                .show(ui, |ui| {
                    ui.set_width(w);
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 2.0;
                        ui.spacing_mut().interact_size.y = 14.0;
                        if group && !m.mine {
                            ui.label(RichText::new(self.display_name(&m.author)).size(12.5).strong().color(theme::color_for(&m.author)));
                        }
                        ui.add(egui::Label::new(RichText::new(text).color(if m.mine { ON_ACCENT() } else { TEXT() })).wrap());
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 6.0;
                            ui.label(RichText::new(self.time(m.ts)).size(11.0).color(if m.mine { ON_ACCENT() } else { TEXT() }.gamma_multiply(0.7)));
                            if m.mine && !icon.is_empty() {
                                ui.label(RichText::new(icon).size(11.0).color(icon_color));
                                if let Some(r) = m.rtt { ui.label(RichText::new(format!("{r} мс")).size(11.0).color(ON_ACCENT().gamma_multiply(0.7))); }
                            }
                        });
                    });
                }).response;
            theme::bubble_decor(ui.painter(), resp.rect, m.mine, &format!("{}{}", m.id, m.ts));
            if m.mine {
                let tip = match m.status.as_str() {
                    "delivered" => format!(
                        "Доставлено и подтверждено получателем (APP_ACK)\nRTT до APP_ACK: {}\nlookup получателя: {}\nтуннель: {}\nпроход по цепочке relay: {}",
                        ms(m.rtt), ms(m.lookup),
                        m.build.map(|b| format!("построен за {b} мс")).unwrap_or_else(|| "уже был установлен".into()),
                        ms(m.send)),
                    "pending" => "Отправляется: lookup → туннель → E2E → ожидание APP_ACK".into(),
                    "queued" => format!("Получатель был недоступен — сообщение лежит в почтовом ящике DHT, он заберёт его сам.\n{}",
                                        m.error.clone().unwrap_or_default()),
                    "sent" => "Отправлено без ожидания подтверждения".into(),
                    _ => format!("Не доставлено: {}", m.error.clone().unwrap_or_default()),
                };
                resp.on_hover_text(tip);
            }
        });
        ui.add_space(2.0);
    }

    // ── Контакты и профиль ─────────────────────────────────────────────────

    fn contacts_tab(&mut self, ui: &mut egui::Ui) {
        ui.heading("Контакты и профиль");
        ui.add_space(10.0);
        let me = self.my_id();
        card(ui, |ui| {
            section_title(ui, "Вы");
            ui.horizontal(|ui| {
                let label = if self.s.my_alias.is_empty() { "Я".to_owned() } else { self.s.my_alias.clone() };
                theme::avatar(ui, if me.is_empty() { "me" } else { &me }, &label, 56.0);
                ui.vertical(|ui| {
                    if !self.s.my_alias.is_empty() { ui.label(RichText::new(format!("@{}", self.s.my_alias)).size(18.0).strong()); }
                    if me.is_empty() { ui.label(RichText::new("узел не подключён").color(MUTED())); }
                    else {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(&me).monospace().size(12.5));
                            if ui.small_button("📋").on_hover_text("Скопировать NodeID").clicked() {
                                ui.ctx().copy_text(me.clone());
                                self.toast("NodeID скопирован — отправьте его собеседнику", GREEN());
                            }
                        });
                        if let Some(st) = &self.status { ui.label(RichText::new(format!("адрес {}", api::s(st, "addr"))).color(MUTED())); }
                    }
                });
            });
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label("@");
                ui.add(field(&mut self.alias_in).hint_text("псевдоним: a-z 0-9 _ -").desired_width(220.0));
                if ui.add_enabled(self.connected() && !self.alias_in.trim().is_empty(), primary("Опубликовать псевдоним")).clicked() {
                    let name = self.alias_in.trim().trim_start_matches('@').to_owned();
                    self.bg(move |p| { let res = api::post(p, "/api/alias", &[("name", &name)]); Msg::Alias { name, res } });
                }
            });
            ui.label(RichText::new("Псевдоним — подписанная запись в DHT: по @имени собеседники найдут ваш NodeID. Занятое имя перезаписать нельзя.").size(12.5).color(MUTED()));
        });
        ui.add_space(12.0);

        card(ui, |ui| {
            section_title(ui, "Приватность");
            let mut changed = ui.checkbox(&mut self.s.send_typing, "Сообщать собеседнику «печатает…» (раз в 4 с, только по открытому туннелю)").changed();
            changed |= ui.checkbox(&mut self.s.send_receipts, "Отправлять отчёты о прочтении").changed();
            if changed { let _ = self.s.save(); }
        });
        ui.add_space(12.0);

        card(ui, |ui| {
            section_title(ui, "Оформление");
            if let Some(id) = theme::picker(ui) {
                self.s.theme = id.to_owned();
                let _ = self.s.save();
                theme::set(ui.ctx(), id);
            }
            ui.label(RichText::new("Вампирские и фейские темы — с оформлением: кровь на панелях, замок и летучие мыши, \
                                    светлячки и мухоморы, цветы, бабочки и пыльца.").size(12.5).color(MUTED()));
        });
        ui.add_space(12.0);

        card(ui, |ui| {
            section_title(ui, "Добавить контакт");
            ui.horizontal(|ui| {
                ui.add(field(&mut self.c_name).hint_text("имя").desired_width(180.0));
                ui.add(field(&mut self.c_id).hint_text("NodeID (64 hex) или @псевдоним").desired_width(ui.available_width() - 130.0));
                if ui.add(primary("Сохранить")).clicked() {
                    let (n, t) = (std::mem::take(&mut self.c_name), std::mem::take(&mut self.c_id));
                    self.with_target(&t, Purpose::AddContact(n));
                }
            });
        });
        ui.add_space(12.0);

        section_title(ui, &format!("Адресная книга · {}", self.s.contacts.len()));
        if self.s.contacts.is_empty() {
            ui.label(RichText::new("Пусто. Контакты хранятся локально в настройках приложения.").color(MUTED()));
        }
        let mut remove = None;
        let mut open = None;
        let mut route = None;
        for (i, c) in self.s.contacts.iter().enumerate() {
            card(ui, |ui| {
                ui.horizontal(|ui| {
                    theme::avatar(ui, &c.node_id, &c.name, 40.0);
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 0.0;
                        ui.label(RichText::new(&c.name).strong());
                        ui.label(RichText::new(&c.node_id).monospace().size(11.5).color(MUTED()));
                    });
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.button("🗑").on_hover_text("Удалить").clicked() { remove = Some(i); }
                        if ui.button("Маршрут").clicked() { route = Some(c.node_id.clone()); }
                        if ui.add(primary("Написать")).clicked() { open = Some(c.node_id.clone()); }
                    });
                });
            });
            ui.add_space(6.0);
        }
        if let Some(i) = remove { self.s.contacts.remove(i); let _ = self.s.save(); }
        if let Some(id) = route { self.request_route(id.clone()); self.open_chat(id); }
        else if let Some(id) = open { self.open_chat(id); }
    }

    // ── Файлы и звонки (интерфейс готов, функция узла — в плане) ───────────

    fn coming_soon(&self, ui: &mut egui::Ui, feature: &str, title: &str, text: &str) {
        let supported = self.feature(feature);
        egui::Frame::default().fill(YELLOW().gamma_multiply(0.08)).stroke(Stroke::new(1.0_f32, YELLOW().gamma_multiply(0.5)))
            .corner_radius(CornerRadius::same(12)).inner_margin(Margin::same(16))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(RichText::new(title).size(16.0).strong().color(YELLOW()));
                ui.label(text);
                ui.label(RichText::new(if supported {
                    format!("Узел сообщает features.{feature} = true, но этот клиент ещё не подключён к API функции.")
                } else if self.connected() {
                    format!("Подключённый узел сообщает features.{feature} = false — элементы ниже неактивны.")
                } else {
                    "Узел не подключён.".to_owned()
                }).size(12.5).color(MUTED()));
            });
        ui.add_space(14.0);
    }

    fn files_tab(&mut self, ui: &mut egui::Ui) {
        ui.heading("Файлы");
        ui.label(RichText::new("Файл режется на блоки по 40 КиБ; каждый блок подписан и зашифрован для получателя, идёт через туннель; \
                                получатель подтверждает окна блоков и в конце проверяет SHA-256.").color(MUTED()));
        ui.add_space(10.0);
        if !self.connected() { self.not_connected_hint(ui); return; }
        // перетаскивание файла в окно
        let dropped: Vec<String> = ui.ctx().input(|i| i.raw.dropped_files.iter().filter_map(|f| f.path.as_ref().map(|p| p.display().to_string())).collect());
        if let Some(p) = dropped.into_iter().next() { self.f_path = p; }
        let hovering = ui.ctx().input(|i| !i.raw.hovered_files.is_empty());
        card(ui, |ui| {
            section_title(ui, "Отправить файл");
            ui.horizontal(|ui| {
                ui.add(field(&mut self.f_target).hint_text("получатель: NodeID или @псевдоним").desired_width(330.0));
                egui::ComboBox::from_id_salt("f_contact").selected_text("из контактов").show_ui(ui, |ui| {
                    for c in &self.s.contacts {
                        if ui.selectable_label(false, &c.name).clicked() { self.f_target = c.node_id.clone(); }
                    }
                });
            });
            ui.horizontal(|ui| {
                ui.add(field(&mut self.f_path).hint_text("путь к файлу на этом компьютере").desired_width(ui.available_width() - 150.0));
                let can = !self.f_path.trim().is_empty() && !self.f_target.trim().is_empty();
                if ui.add_enabled(can, primary("Отправить →")).clicked() {
                    let (path, target) = (self.f_path.trim().to_owned(), self.f_target.trim().to_owned());
                    let name = std::path::Path::new(&path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    self.f_sending.push(name.clone());
                    self.bg(move |p| {
                        let res = api::call(p, "POST", "/api/send_file", &[("target", &target), ("path", &path)], Duration::from_secs(1800));
                        Msg::FileSent { name, res }
                    });
                }
            });
            let (r, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 90.0), egui::Sense::hover());
            let c = if hovering { ACCENT() } else { MUTED().gamma_multiply(0.5) };
            ui.painter().rect_stroke(r, CornerRadius::same(12), Stroke::new(1.5_f32, c), egui::StrokeKind::Inside);
            ui.painter().text(r.center(), egui::Align2::CENTER_CENTER,
                if hovering { "Отпустите — путь подставится в поле выше" } else { "…или перетащите файл в окно" },
                egui::FontId::proportional(15.0), c);
            if !self.f_sending.is_empty() {
                ui.label(RichText::new(format!("⏳ отправляется: {}", self.f_sending.join(", "))).color(YELLOW()));
            }
        });
        ui.add_space(12.0);
        card(ui, |ui| {
            section_title(ui, "Передачи");
            if let Some(d) = self.files["downloads_dir"].as_str() {
                ui.label(RichText::new(format!("Полученные файлы сохраняются в {d}")).size(12.5).color(MUTED()));
            }
            let list = self.files["transfers"].as_array().cloned().unwrap_or_default();
            if list.is_empty() { ui.label(RichText::new("передач пока не было").color(MUTED())); return; }
            egui::Grid::new("files_grid").num_columns(6).striped(true).spacing([18.0, 8.0]).show(ui, |ui| {
                for h in ["", "Файл", "Собеседник", "Размер", "Прогресс", "Статус"] { ui.label(RichText::new(h).strong()); }
                ui.end_row();
                for t in list.iter().rev() {
                    ui.label(if api::s(t, "dir") == "out" { "↗" } else { "↘" });
                    let name = api::s(t, "name");
                    let path = t["path"].as_str().unwrap_or_default().to_owned();
                    ui.label(&name).on_hover_text(if path.is_empty() { name.clone() } else { path });
                    ui.label(self.display_name(&api::s(t, "peer")));
                    let size = api::u(t, "size").unwrap_or(0) as f64;
                    ui.label(if size >= 1048576.0 { format!("{:.1} МиБ", size / 1048576.0) } else { format!("{:.0} КиБ", size / 1024.0) });
                    let (done, all) = (api::u(t, "done_blocks").unwrap_or(0) as f32, api::u(t, "blocks").unwrap_or(1).max(1) as f32);
                    ui.add(egui::ProgressBar::new(done / all).desired_width(160.0).text(format!("{}/{}", done as u64, all as u64)));
                    let st = api::s(t, "status");
                    let c = match st.as_str() { "sent" | "received" => GREEN(), "failed" | "rejected" => RED(), _ => YELLOW() };
                    let label = match st.as_str() { "sent" => "доставлен", "received" => "получен", "sending" => "отправка",
                                                     "receiving" => "приём", "rejected" => "отклонён", "offered" => "ждёт вашего решения",
                                                     "waiting" => "ждёт согласия получателя", _ => "ошибка" };
                    if st == "offered" {
                        let id = api::s(t, "transfer_id");
                        ui.horizontal(|ui| {
                            if ui.add(primary("Принять")).clicked() { self.file_decision("/api/file/accept", id.clone()); }
                            if ui.add(danger("Отклонить")).clicked() { self.file_decision("/api/file/reject", id.clone()); }
                        });
                    } else {
                        let resp = theme::pill_resp(ui, label, c);
                        if let Some(e) = t["error"].as_str() { resp.on_hover_text(e); }
                    }
                    ui.end_row();
                }
            });
        });
    }

    // ── Звонки ─────────────────────────────────────────────────────────────

    /// Текущий сеанс (вызов или разговор).
    fn live_call(&self) -> Option<Value> {
        self.calls["sessions"].as_array()?.iter().find(|s| s["state"] != "ended").cloned()
    }

    fn call_title(&self, s: &Value) -> String {
        match s["group"].as_object() {
            Some(g) => format!("👥 {}", g.get("name").and_then(Value::as_str).unwrap_or("Групповой звонок")),
            None => self.display_name(&api::s(&s["peers"][0], "peer")),
        }
    }

    /// Звук включается, когда звонок наш (исходящий или принятый), и выключается по его окончании.
    fn sync_call_audio(&mut self) {
        let want = self.live_call().filter(|s| !(s["state"] == "ringing" && s["outgoing"] != true)).map(|s| api::s(&s, "id"));
        if want == self.call_sid { return; }
        self.call_audio = None;
        self.call_sid = want.clone();
        self.call_err = None;
        let Some(_) = want else { return };
        let p = self.port.load(Ordering::Relaxed);
        match api::ok(api::get(p, "/api/call/audio", &[])) {
            Ok(v) => {
                let token = hex_bytes(&api::s(&v, "token"));
                match crate::voice::CallAudio::start(api::u(&v, "port").unwrap_or(0) as u16, token) {
                    Ok(a) => self.call_audio = Some(a),
                    Err(e) => { self.call_err = Some(e.clone()); self.toast(format!("Звук: {e}"), RED()); }
                }
            }
            Err(e) => self.call_err = Some(e),
        }
    }

    fn start_call(&mut self, peer: String) {
        let me = self.my_id();
        let group = self.groups.get(&peer).cloned();
        self.tab = Tab::Calls;
        self.bg(move |p| {
            let res = match group {
                Some((name, members)) => {
                    let m: Vec<String> = members.into_iter().filter(|x| *x != me).collect();
                    let gid = peer.trim_start_matches("g:").to_owned();
                    api::call(p, "POST", "/api/call/start", &[("members", &m.join(",")), ("name", &name), ("chat", &gid)], Duration::from_secs(60))
                }
                None => api::call(p, "POST", "/api/call/start", &[("target", &peer)], Duration::from_secs(60)),
            };
            Msg::CallDone { what: "Звонок".into(), res }
        });
    }

    fn call_action(&self, path: &'static str, sid: String) {
        self.bg(move |p| {
            let res = api::call(p, "POST", path, &[("call_id", &sid)], Duration::from_secs(60));
            Msg::CallDone { what: "Звонок".into(), res }
        });
    }

    /// Полоса над окном: входящий вызов (ответить / отклонить) или идущий звонок вне вкладки «Звонки».
    fn call_banner(&mut self, ctx: &egui::Context) {
        let Some(s) = self.live_call() else { return };
        let sid = api::s(&s, "id");
        let incoming = s["state"] == "ringing" && s["outgoing"] != true;
        if !incoming && self.tab == Tab::Calls { return; }
        if incoming && self.call_attention.as_deref() != Some(sid.as_str()) {
            self.call_attention = Some(sid.clone());
            ctx.send_viewport_cmd(egui::ViewportCommand::RequestUserAttention(egui::UserAttentionType::Critical));
        }
        let fill = if incoming { ACCENT_DIM() } else { GREEN().gamma_multiply(0.75) };
        egui::TopBottomPanel::top("call_banner").frame(egui::Frame::default().fill(fill).inner_margin(Margin::symmetric(18, 10)))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("📞").size(20.0));
                    let title = self.call_title(&s);
                    let what = if incoming {
                        if s["group"].is_object() { "входящий групповой звонок".to_owned() } else { "входящий звонок".to_owned() }
                    } else { call_status(&s) };
                    ui.label(RichText::new(format!("{title} — {what}")).size(15.0).strong().color(Color32::WHITE));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if incoming {
                            if ui.add(danger("Отклонить")).clicked() { self.call_action("/api/call/reject", sid.clone()); }
                            if ui.add(egui::Button::new(RichText::new("Ответить").color(Color32::WHITE)).fill(GREEN())).clicked() {
                                self.call_action("/api/call/accept", sid.clone());
                                self.tab = Tab::Calls;
                            }
                        } else {
                            if ui.add(danger("Завершить")).clicked() { self.call_action("/api/call/hangup", sid.clone()); }
                            if ui.button("Открыть").clicked() { self.tab = Tab::Calls; }
                        }
                    });
                });
            });
    }

    fn file_decision(&self, path: &'static str, id: String) {
        self.bg(move |p| { let res = api::post(p, path, &[("transfer_id", &id)]); Msg::CallDone { what: "Файл".into(), res } });
    }

    fn calls_tab(&mut self, ui: &mut egui::Ui) {
        ui.heading("Звонки");
        ui.label(RichText::new("Голос идёт через туннель из ≥2 ретрансляторов: Opus 16 кГц, свой ключ на каждый звонок (X25519 → \
                                AES-256-GCM), кадры без подтверждений; групповой — «каждый с каждым», до 5 участников.").color(MUTED()));
        ui.add_space(10.0);
        if !self.connected() { self.not_connected_hint(ui); return; }
        if !self.feature("calls") {
            self.coming_soon(ui, "calls", "📞 Звонки", "Подключённый узел звонки не поддерживает — обновите p2p-node.");
            return;
        }
        match self.live_call() {
            Some(s) => self.call_view(ui, &s),
            None => self.call_start_view(ui),
        }
    }

    fn call_view(&mut self, ui: &mut egui::Ui, s: &Value) {
        let sid = api::s(s, "id");
        let incoming = s["state"] == "ringing" && s["outgoing"] != true;
        card(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(self.call_title(s)).size(22.0).strong());
                theme::pill(ui, &call_status(s), if s["state"] == "active" { GREEN() } else { YELLOW() });
                if let Some(m) = s["mos"].as_f64() {
                    theme::pill_resp(ui, &format!("MOS {m:.1}"), if m >= 4.0 { GREEN() } else if m >= 3.6 { YELLOW() } else { RED() })
                        .on_hover_text("Оценка качества по E-модели ITU-T G.107: задержка «рот — ухо» и слышимые потери");
                }
            });
            ui.add_space(12.0);
            let me = self.my_id();
            let mut tiles: Vec<(String, String, f32, String, Value)> = vec![(me.clone(), "Вы".into(),
                self.call_audio.as_ref().map_or(0.0, |a| a.level()), "active".into(), Value::Null)];
            for p in s["peers"].as_array().cloned().unwrap_or_default() {
                if p["state"] == "ended" && s["state"] != "ended" { continue; }
                let id = api::s(&p, "peer");
                tiles.push((id.clone(), self.display_name(&id), p["level"].as_f64().unwrap_or(0.0) as f32, api::s(&p, "state"), p["quality"].clone()));
            }
            ui.horizontal_wrapped(|ui| {
                for (id, name, level, st, q) in tiles {
                    let speaking = level > 0.02;
                    egui::Frame::default().fill(BG_MAIN()).corner_radius(CornerRadius::same(14)).inner_margin(Margin::same(16))
                        .stroke(Stroke::new(if speaking { 3.0_f32 } else { 1.0_f32 }, if speaking { GREEN() } else { BG_RAIL() }))
                        .show(ui, |ui| {
                            ui.set_width(160.0);
                            ui.vertical_centered(|ui| {
                                theme::avatar(ui, &id, &name, 64.0);
                                ui.label(RichText::new(&name).strong());
                                let sub = match st.as_str() {
                                    "ringing" => "вызов…".to_owned(),
                                    _ if id == me => if self.call_audio.as_ref().is_some_and(|a| a.muted.load(Ordering::Relaxed)) { "🔇 микрофон выкл.".into() } else { "🎤".into() },
                                    _ => match (q["mouth_to_ear_ms"].as_f64(), q["loss_audible_pct"].as_f64()) {
                                        (Some(d), Some(l)) => format!("{d:.0} мс · потери {l:.1}%"),
                                        _ => "соединение…".into(),
                                    },
                                };
                                ui.label(RichText::new(sub).size(12.0).color(MUTED()));
                            });
                        });
                }
            });
        });
        ui.add_space(12.0);
        ui.horizontal(|ui| {
            if incoming {
                if ui.add_sized([170.0, 44.0], egui::Button::new(RichText::new("📞 Ответить").color(Color32::WHITE)).fill(GREEN())).clicked() {
                    self.call_action("/api/call/accept", sid.clone());
                }
                if ui.add_sized([150.0, 44.0], danger("Отклонить")).clicked() { self.call_action("/api/call/reject", sid.clone()); }
            } else {
                if let Some(a) = &self.call_audio {
                    let m = a.muted.load(Ordering::Relaxed);
                    if ui.add_sized([150.0, 44.0], egui::Button::new(if m { "🔇 Включить микрофон" } else { "🎤 Выключить микрофон" })).clicked() {
                        a.muted.store(!m, Ordering::Relaxed);
                    }
                }
                if ui.add_sized([150.0, 44.0], danger("⛔ Завершить")).clicked() { self.call_action("/api/call/hangup", sid.clone()); }
            }
        });
        if let Some(a) = &self.call_audio { ui.label(RichText::new(&a.devices).size(12.0).color(MUTED())); }
        if let Some(e) = &self.call_err { ui.label(RichText::new(format!("Звук: {e}")).color(RED())); }
    }

    fn call_start_view(&mut self, ui: &mut egui::Ui) {
        let mut call = None;
        card(ui, |ui| {
            section_title(ui, "Позвонить");
            let mut targets: Vec<(String, String)> = self.s.contacts.iter().map(|c| (c.node_id.clone(), c.name.clone())).collect();
            for (g, (name, members)) in &self.groups {
                if members.len() <= 5 { targets.push((g.clone(), format!("👥 {name} ({})", members.len()))); }
            }
            if targets.is_empty() { ui.label(RichText::new("Нет контактов и групп — добавьте собеседника на вкладке «Контакты».").color(MUTED())); }
            for (id, name) in targets {
                ui.horizontal(|ui| {
                    theme::avatar(ui, &id, &name, 32.0);
                    ui.label(RichText::new(&name).strong());
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.add(primary("📞 Позвонить")).clicked() { call = Some(id.clone()); }
                    });
                });
            }
        });
        if let Some(id) = call { self.start_call(id); }
        ui.add_space(12.0);
        card(ui, |ui| {
            section_title(ui, "Недавние звонки");
            let recent: Vec<&Value> = self.history.iter().rev().filter(|e| api::s(e, "status").starts_with("call_")).take(15).collect();
            if recent.is_empty() { ui.label(RichText::new("звонков пока не было").color(MUTED())); }
            for e in recent {
                ui.horizontal(|ui| {
                    let c = match api::s(e, "status").as_str() { "call_ended" => GREEN(), "call_missed" => RED(), _ => YELLOW() };
                    ui.label(RichText::new(if api::s(e, "dir") == "out" { "↗" } else { "↘" }).color(c));
                    ui.label(api::s(e, "text"));
                    ui.label(RichText::new(self.display_name(&api::s(e, "peer"))).color(MUTED()));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.label(RichText::new(self.time(api::u(e, "ts_ms").unwrap_or(0))).size(12.0).color(MUTED()));
                    });
                });
            }
        });
    }

    // ── Сеть ───────────────────────────────────────────────────────────────

    fn network_tab(&mut self, ui: &mut egui::Ui) {
        ui.heading("Сеть");
        ui.add_space(10.0);
        let Some(st) = self.status.clone() else { self.not_connected_hint(ui); return; };
        let routing = &st["routing"];
        let tunnels = st["tunnels"]["tunnels"].as_array().cloned().unwrap_or_default();
        let records = st["store"]["records"].as_array().cloned().unwrap_or_default();
        ui.horizontal(|ui| {
            for (v, l) in [
                (api::s(routing, "total_contacts"), "контактов"),
                (api::s(routing, "non_empty_buckets"), "непустых бакетов"),
                (tunnels.len().to_string(), "туннелей"),
                (records.len().to_string(), "записей DHT"),
                (st["bootstrap"]["join_ms"].as_u64().map(|x| if x == 0 { "seed".into() } else { format!("{x} мс") }).unwrap_or("—".into()), "bootstrap"),
            ] {
                egui::Frame::default().fill(BG_CARD()).corner_radius(CornerRadius::same(12)).inner_margin(Margin::same(14))
                    .show(ui, |ui| {
                        ui.vertical(|ui| {
                            ui.set_min_width(120.0);
                            ui.label(RichText::new(v).size(26.0).strong());
                            ui.label(RichText::new(l).color(MUTED()));
                        });
                    });
            }
        });
        ui.add_space(12.0);
        card(ui, |ui| {
            section_title(ui, "Узел");
            egui::Grid::new("node_info").num_columns(2).spacing([16.0, 6.0]).show(ui, |ui| {
                ui.label("NodeID"); ui.label(RichText::new(api::s(&st, "node_id")).monospace().size(12.5)); ui.end_row();
                ui.label("Адрес"); ui.label(api::s(&st, "addr")); ui.end_row();
                ui.label("Уровень"); ui.label(format!("{} · K={} · α={}", api::s(&st, "level"), api::s(&st, "k"), api::s(&st, "alpha"))); ui.end_row();
            });
        });
        ui.add_space(12.0);

        card(ui, |ui| {
            section_title(ui, "Инструменты");
            ui.horizontal(|ui| {
                ui.add(field(&mut self.t_ping).hint_text("127.0.0.1:7001").desired_width(200.0));
                if ui.button("PING").clicked() {
                    let a = self.t_ping.trim().to_owned();
                    self.bg(move |p| Msg::Tool { title: format!("PING {a}"), res: api::post(p, "/api/ping", &[("addr", &a)]) });
                }
            });
            ui.horizontal(|ui| {
                ui.add(field(&mut self.t_lookup).hint_text("NodeID цели").desired_width(420.0));
                if ui.button("FIND_NODE").clicked() {
                    let t = self.t_lookup.trim().to_owned();
                    self.bg(move |p| Msg::Tool { title: format!("lookup {}", short(&t)), res: api::post(p, "/api/lookup", &[("target", &t)]) });
                }
            });
            ui.horizontal(|ui| {
                ui.add(field(&mut self.t_route).hint_text("NodeID или @псевдоним").desired_width(420.0));
                if ui.button("Маршрут").clicked() {
                    let t = self.t_route.trim().to_owned();
                    self.bg(move |p| Msg::Tool { title: format!("маршрут {t}"), res: api::get(p, "/api/route", &[("target", &t)]) });
                }
            });
            ui.horizontal(|ui| {
                ui.add(field(&mut self.t_key).hint_text("ключ").desired_width(160.0));
                ui.add(field(&mut self.t_val).hint_text("значение").desired_width(250.0));
                if ui.button("STORE").clicked() {
                    let (k, v) = (self.t_key.clone(), self.t_val.clone());
                    self.bg(move |p| Msg::Tool { title: format!("STORE {k}"), res: api::post(p, "/api/store", &[("key", &k), ("value", &v)]) });
                }
                if ui.button("FIND_VALUE").clicked() {
                    let k = self.t_key.clone();
                    self.bg(move |p| Msg::Tool { title: format!("FIND_VALUE {k}"), res: api::get(p, "/api/find", &[("key", &k)]) });
                }
            });
            if let Some((title, res)) = &self.tool_out {
                ui.add_space(6.0);
                let (c, body) = match res {
                    Ok(v) if v.get("ok").and_then(Value::as_bool) != Some(false) => (GREEN(), serde_json::to_string_pretty(v).unwrap_or_default()),
                    Ok(v) => (RED(), serde_json::to_string_pretty(v).unwrap_or_default()),
                    Err(e) => (RED(), e.clone()),
                };
                ui.label(RichText::new(title).color(c).strong());
                egui::Frame::default().fill(BG_INPUT()).corner_radius(CornerRadius::same(8)).inner_margin(Margin::same(10)).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ScrollArea::vertical().id_salt("tool_out").max_height(220.0).show(ui, |ui| {
                        ui.label(RichText::new(body).monospace());
                    });
                });
            }
        });
        ui.add_space(12.0);

        let now = api::u(&st, "now_ms").unwrap_or_else(now_ms);
        card(ui, |ui| {
            section_title(ui, &format!("Туннели · {}", tunnels.len()));
            if tunnels.is_empty() { ui.label(RichText::new("нет — туннель строится при первой отправке").color(MUTED())); }
            egui::Grid::new("tunnels").num_columns(6).striped(true).spacing([24.0, 6.0]).show(ui, |ui| {
                if !tunnels.is_empty() {
                    for h in ["tunnel_id", "состояние", "роль", "hops", "посл. ACK", "uptime"] { ui.label(RichText::new(h).strong()); }
                    ui.end_row();
                }
                for t in &tunnels {
                    ui.label(RichText::new(api::s(t, "tunnel_id")).monospace());
                    let state = api::s(t, "state");
                    theme::pill(ui, &state, state_color(&state));
                    ui.label(format!("{} → {}…", if api::s(t, "role") == "backup" { "резерв" } else { "основной" }, api::s(t, "target")));
                    ui.label(api::s(t, "hops"));
                    ui.label(api::u(t, "last_ack_age_ms").map(|x| format!("{} назад", ago(x))).unwrap_or("—".into()));
                    ui.label(api::u(t, "uptime_ms").map(ago).unwrap_or_default());
                    ui.end_row();
                }
            });
        });
        let profiles = self.relays["profiles"].as_array().cloned().unwrap_or_default();
        if !profiles.is_empty() {
            ui.add_space(12.0);
            card(ui, |ui| {
                section_title(ui, &format!("Профили ретрансляторов · выбор: {} · пул: {}", api::s(&st["tunnels"], "relay_selection"),
                                           api::s(&st["tunnels"], "pool_size")));
                egui::Grid::new("relays").num_columns(6).striped(true).spacing([20.0, 6.0]).show(ui, |ui| {
                    for h in ["узел", "оценка", "успехов", "отказов", "подряд отказов", "EMA задержки"] { ui.label(RichText::new(h).strong()); }
                    ui.end_row();
                    for p in profiles.iter().take(12) {
                        let id = api::s(p, "node_id");
                        ui.label(RichText::new(format!("{}…", short(&id))).monospace()).on_hover_text(&id);
                        let sc = p["score"].as_f64().unwrap_or(0.0);
                        ui.label(RichText::new(format!("{sc:.2}")).color(if sc > 0.6 { GREEN() } else if sc > 0.3 { YELLOW() } else { RED() }));
                        let pr = &p["profile"];
                        ui.label(api::s(pr, "successes"));
                        ui.label(api::s(pr, "failures"));
                        ui.label(api::s(pr, "consecutive_failures"));
                        ui.label(format!("{:.0} мс", pr["ema_latency_ms"].as_f64().unwrap_or(0.0)));
                        ui.end_row();
                    }
                });
            });
        }
        ui.add_space(12.0);
        card(ui, |ui| {
            section_title(ui, &format!("Таблица маршрутизации · {} контактов", api::s(routing, "total_contacts")));
            egui::Grid::new("rt").num_columns(4).striped(true).spacing([24.0, 6.0]).show(ui, |ui| {
                for h in ["бакет", "NodeID", "адрес", "виден"] { ui.label(RichText::new(h).strong()); }
                ui.end_row();
                for b in routing["buckets"].as_array().cloned().unwrap_or_default() {
                    for c in b["contacts"].as_array().cloned().unwrap_or_default() {
                        ui.label(api::s(&b, "bucket_index"));
                        let id = api::s(&c, "node_id");
                        ui.label(RichText::new(format!("{}…", short(&id))).monospace()).on_hover_text(&id);
                        ui.label(format!("{}:{}", api::s(&c, "host"), api::s(&c, "port")));
                        ui.label(api::u(&c, "last_seen_ms").map(|x| format!("{} назад", ago(now.saturating_sub(x)))).unwrap_or_default());
                        ui.end_row();
                    }
                }
            });
        });
        ui.add_space(12.0);
        card(ui, |ui| {
            section_title(ui, &format!("Хранилище DHT · {}", records.len()));
            egui::Grid::new("store").num_columns(4).striped(true).spacing([24.0, 6.0]).show(ui, |ui| {
                for h in ["ключ", "публикатор", "TTL", "байт"] { ui.label(RichText::new(h).strong()); }
                ui.end_row();
                for r in &records {
                    let k = api::s(r, "key");
                    ui.label(RichText::new(format!("{}…", short(&k))).monospace()).on_hover_text(&k);
                    ui.label(RichText::new(format!("{}…", short(&api::s(r, "publisher")))).monospace());
                    ui.label(format!("{} с", api::s(r, "remaining_secs")));
                    ui.label(api::s(r, "value_len"));
                    ui.end_row();
                }
            });
        });
    }

    // ── Журнал ─────────────────────────────────────────────────────────────

    fn journal_tab(&mut self, ui: &mut egui::Ui) {
        ui.heading("Журнал отправки и приёма");
        ui.label(RichText::new("Локальная история узла (state/history.jsonl): переживает перезапуск.").color(MUTED()));
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            for (i, l) in ["Все", "Исходящие", "Входящие", "Ошибки"].iter().enumerate() {
                if ui.selectable_label(self.j_filter == i, *l).clicked() { self.j_filter = i; }
            }
        });
        ui.add_space(8.0);
        if !self.connected() { self.not_connected_hint(ui); return; }
        card(ui, |ui| {
            egui::Grid::new("journal").num_columns(6).striped(true).spacing([18.0, 6.0]).show(ui, |ui| {
                for h in ["время", "", "собеседник", "статус", "RTT", "текст / ошибка"] { ui.label(RichText::new(h).strong()); }
                ui.end_row();
                for e in self.history.iter().rev() {
                    let (dir, status) = (api::s(e, "dir"), api::s(e, "status"));
                    let keep = match self.j_filter { 1 => dir == "out", 2 => dir == "in", 3 => status == "failed", _ => true };
                    if !keep { continue; }
                    ui.label(self.time(api::u(e, "ts_ms").unwrap_or(0)));
                    ui.label(if dir == "out" { "↗" } else { "↘" });
                    let peer = api::s(e, "peer");
                    ui.label(self.display_name(&peer)).on_hover_text(&peer);
                    let c = match status.as_str() { "delivered" | "received" => GREEN(), "failed" => RED(), _ => YELLOW() };
                    theme::pill(ui, &status, c);
                    ui.label(ms(api::u(e, "rtt_ms")));
                    let body = e["error"].as_str().map(str::to_owned).unwrap_or_else(|| api::s(e, "text"));
                    let shown: String = body.chars().take(80).collect();
                    ui.label(shown).on_hover_text(body);
                    ui.end_row();
                }
            });
        });
    }

    // ── Подключение ────────────────────────────────────────────────────────

    fn connection_tab(&mut self, ui: &mut egui::Ui) {
        ui.heading("Подключение к узлу");
        ui.add_space(10.0);
        card(ui, |ui| {
            section_title(ui, "Режим");
            ui.radio_value(&mut self.s.mode, Mode::Launch, "Запустить свой узел из приложения");
            ui.radio_value(&mut self.s.mode, Mode::Attach, "Подключиться к уже работающему узлу (p2p-node --gui-port …)");
            ui.add_space(8.0);
            egui::Grid::new("conn").num_columns(2).spacing([16.0, 8.0]).show(ui, |ui| {
                match self.s.mode {
                    Mode::Launch => {
                        ui.label("Программа узла");
                        ui.vertical(|ui| {
                            ui.add(field(&mut self.s.node_bin).hint_text("авто").desired_width(420.0));
                            ui.label(RichText::new(format!("будет использовано: {}", find_node_binary(&self.s.node_bin).display())).size(12.0).color(MUTED()));
                        });
                        ui.end_row();
                        ui.label("Порт узла (P2P)"); ui.add(egui::DragValue::new(&mut self.s.listen_port).range(1024..=65535)); ui.end_row();
                        ui.label("Порт API (127.0.0.1)"); ui.add(egui::DragValue::new(&mut self.s.gui_port).range(1024..=65535)); ui.end_row();
                        ui.label("Bootstrap-пиры");
                        ui.vertical(|ui| {
                            ui.add(field(&mut self.s.bootstrap).hint_text("127.0.0.1:7001, …").desired_width(420.0));
                            ui.label(RichText::new("пусто — узел станет первым (seed) узлом сети").size(12.0).color(MUTED()));
                        });
                        ui.end_row();
                        ui.label("Каталог состояния");
                        ui.vertical(|ui| {
                            ui.add(field(&mut self.s.state_dir).desired_width(420.0));
                            ui.label(RichText::new("identity.key (= ваш NodeID), history.jsonl, журнал узла").size(12.0).color(MUTED()));
                        });
                        ui.end_row();
                        ui.label("Автозапуск"); ui.checkbox(&mut self.s.autostart, "запускать узел при старте приложения"); ui.end_row();
                        ui.label("Локальная сеть");
                        ui.vertical(|ui| {
                            ui.checkbox(&mut self.s.lan, "доступен с других устройств (телефон, другой ПК)");
                            ui.label(RichText::new("узел сообщает пирам свой LAN-адрес (--advertise auto); порт P2P должен быть открыт в брандмауэре")
                                .size(12.0).color(MUTED()));
                        });
                        ui.end_row();
                        ui.label("Ретрансляторы");
                        ui.vertical(|ui| {
                            ui.add(egui::DragValue::new(&mut self.s.helpers).range(0..=6).suffix(" шт."));
                            ui.label(RichText::new(format!(
                                "запустить рядом вспомогательные узлы (порты {}…): туннелю нужно ≥ 2 ретранслятора, \
                                 а в сети «телефон + компьютер» их больше нет", self.s.listen_port.saturating_add(1)))
                                .size(12.0).color(MUTED()));
                        });
                        ui.end_row();
                    }
                    Mode::Attach => {
                        ui.label("Порт API узла"); ui.add(egui::DragValue::new(&mut self.s.attach_port).range(1..=65535)); ui.end_row();
                    }
                }
            });
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let active = self.port.load(Ordering::Relaxed) != 0;
                let label = match self.s.mode { Mode::Launch => "▶ Запустить узел", Mode::Attach => "🔗 Подключиться" };
                if ui.add(primary(if active { "↻ Переподключить" } else { label })).clicked() { self.connect(); }
                if active && ui.add(danger(if self.local.is_some() { "■ Остановить узел" } else { "Отключиться" })).clicked() {
                    self.disconnect();
                }
                if ui.button("💾 Сохранить").clicked() {
                    match self.s.save() { Ok(()) => self.toast("Настройки сохранены", GREEN()), Err(e) => self.toast(e, RED()) }
                }
            });
            ui.add_space(4.0);
            let line = if self.connected() { (GREEN(), format!("на связи: {}", self.my_id())) }
                else if let Some(e) = &self.conn_err { (YELLOW(), e.clone()) }
                else { (MUTED(), "не подключено".into()) };
            ui.label(RichText::new(line.1).color(line.0).size(12.5));
        });

        if self.connected() { self.phone_card(ui); }

        if let Some(n) = &self.local {
            if self.log_at.elapsed() > Duration::from_millis(1500) {
                self.node_log = n.log_tail(300);
                self.log_at = Instant::now();
            }
            ui.add_space(12.0);
            card(ui, |ui| {
                section_title(ui, &format!("Журнал узла · pid {}", n.pid()));
                ui.label(RichText::new(format!("{}  →  {}", n.bin.display(), n.log_path.display())).size(12.0).color(MUTED()));
                egui::Frame::default().fill(BG_INPUT()).corner_radius(CornerRadius::same(8)).inner_margin(Margin::same(10)).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ScrollArea::vertical().id_salt("node_log").max_height(320.0).stick_to_bottom(true).show(ui, |ui| {
                        for l in &self.node_log {
                            let c = if l.contains(" WARN ") { YELLOW() } else if l.contains(" ERROR ") { RED() } else { MUTED() };
                            ui.label(RichText::new(l).monospace().size(11.5).color(c));
                        }
                    });
                });
            });
        }
    }

    /// Подключение телефона: QR со ссылкой-приглашением (адрес для bootstrap + NodeID).
    fn phone_card(&mut self, ui: &mut egui::Ui) {
        let link = self.status.as_ref().map(|s| api::s(s, "link")).unwrap_or_default();
        if link.is_empty() { return; }
        if self.qr.as_ref().map(|q| q.0 != link).unwrap_or(true) {
            self.qr = qrcode::QrCode::new(link.as_bytes()).ok().map(|c| {
                let w = c.width();
                (link.clone(), w, c.to_colors().into_iter().map(|m| m == qrcode::Color::Dark).collect())
            });
        }
        let local_only = link.contains("://127.");
        ui.add_space(12.0);
        card(ui, |ui| {
            section_title(ui, "Подключить телефон");
            ui.horizontal_top(|ui| {
                if let Some((_, w, cells)) = &self.qr {
                    let px = 5.0;
                    let quiet = 4.0 * px;
                    let side = *w as f32 * px + 2.0 * quiet;
                    let (rect, _) = ui.allocate_exact_size(Vec2::splat(side), egui::Sense::hover());
                    let p = ui.painter();
                    p.rect_filled(rect, CornerRadius::same(8), Color32::WHITE);
                    for (i, dark) in cells.iter().enumerate() {
                        if !*dark { continue; }
                        let (x, y) = ((i % w) as f32, (i / w) as f32);
                        let min = rect.min + Vec2::new(quiet + x * px, quiet + y * px);
                        p.rect_filled(egui::Rect::from_min_size(min, Vec2::splat(px)), CornerRadius::ZERO, Color32::BLACK);
                    }
                }
                ui.add_space(12.0);
                ui.vertical(|ui| {
                    ui.label("В приложении на Android: вкладка «Узел» → «Сканировать QR» (или вставьте ссылку).");
                    ui.label(RichText::new("Телефон возьмёт этот узел как bootstrap и добавит его в контакты.").color(MUTED()).size(13.0));
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(&link).monospace().size(12.0));
                        if ui.small_button("Копировать").clicked() {
                            ui.ctx().copy_text(link.clone());
                            self.toast("Ссылка скопирована", GREEN());
                        }
                    });
                    ui.add_space(6.0);
                    if local_only {
                        pill(ui, "узел анонсирует 127.0.0.1 — включите «Локальная сеть» и перезапустите", YELLOW());
                    } else {
                        pill(ui, "доступен в локальной сети", GREEN());
                    }
                    let relays = self.helpers.len();
                    let peers = self.status.as_ref().and_then(|s| s["routing"]["total_contacts"].as_u64()).unwrap_or(0) as usize;
                    ui.label(RichText::new(format!("ретрансляторов запущено приложением: {relays}; известно пиров: {peers}"))
                        .size(12.5).color(if relays + peers >= 3 { MUTED() } else { YELLOW() }));
                });
            });
        });
    }

    fn toasts_ui(&mut self, ctx: &egui::Context) {
        self.toasts.retain(|t| t.at.elapsed() < Duration::from_secs(5));
        if self.toasts.is_empty() { return; }
        egui::Area::new(egui::Id::new("toasts")).anchor(egui::Align2::RIGHT_BOTTOM, Vec2::new(-18.0, -18.0))
            .show(ctx, |ui| {
                for t in self.toasts.iter().rev().take(4) {
                    egui::Frame::default().fill(BG_SIDE()).stroke(Stroke::new(1.0_f32, t.color))
                        .corner_radius(CornerRadius::same(10)).inner_margin(Margin::symmetric(14, 10))
                        .show(ui, |ui| { ui.set_max_width(380.0); ui.label(RichText::new(&t.text).color(TEXT())); });
                    ui.add_space(6.0);
                }
            });
    }
}

/// Иконка вкладки на левой панели: подсветка выбранной, счётчик непрочитанного, метка «скоро».
fn rail_button(ui: &mut egui::Ui, icon: &str, selected: bool, badge: usize, soon: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(48.0), egui::Sense::click());
    let p = ui.painter();
    let fill = if selected { ACCENT() } else if resp.hovered() { BG_HOVER() } else { BG_SIDE() };
    p.rect_filled(rect, CornerRadius::same(if selected || resp.hovered() { 16 } else { 24 }), fill);
    if selected {
        let bar = egui::Rect::from_min_size(egui::pos2(rect.left() - 14.0, rect.top() + 8.0), Vec2::new(4.0, 32.0));
        p.rect_filled(bar, CornerRadius::same(2), TEXT());
    }
    let c = if selected { ON_ACCENT() } else if soon { MUTED() } else { TEXT() };
    p.text(rect.center(), egui::Align2::CENTER_CENTER, icon, egui::FontId::proportional(20.0), c);
    if badge > 0 {
        let center = rect.right_bottom() - Vec2::new(6.0, 6.0);
        p.circle_filled(center, 9.0, RED());
        p.text(center, egui::Align2::CENTER_CENTER, badge.min(99).to_string(), egui::FontId::proportional(11.0), Color32::WHITE);
    } else if soon {
        let center = rect.right_top() + Vec2::new(-6.0, 6.0);
        p.circle_filled(center, 5.0, YELLOW());
    }
    resp
}

/// Цепочка «Вы → relay → relay → получатель» по ответу `/api/route`.
fn route_chain(ui: &mut egui::Ui, r: &Result<Value, String>) {
    let v = match r { Ok(v) => v, Err(e) => { ui.label(RichText::new(e).color(RED())); return; } };
    let t = &v["tunnel"];
    if t.is_null() {
        ui.label(RichText::new("Туннеля сейчас нет — он будет построен при следующей отправке.").color(MUTED()));
        return;
    }
    let chip = |ui: &mut egui::Ui, title: &str, sub: &str, c: Color32| {
        egui::Frame::default().fill(c.gamma_multiply(0.12)).stroke(Stroke::new(1.0_f32, c.gamma_multiply(0.7)))
            .corner_radius(CornerRadius::same(10)).inner_margin(Margin::symmetric(10, 6))
            .show(ui, |ui| {
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    ui.label(RichText::new(title).strong().color(c));
                    if !sub.is_empty() { ui.label(RichText::new(sub).size(11.0).color(MUTED())); }
                });
            });
    };
    ui.horizontal_wrapped(|ui| {
        chip(ui, "Вы", "инициатор", GREEN());
        for h in t["hops"].as_array().cloned().unwrap_or_default() {
            ui.label(RichText::new("→").size(18.0).color(MUTED()));
            let alive = h["alive"].as_bool().unwrap_or(false);
            chip(ui, &format!("relay {}", short(&api::s(&h, "node_id"))), &api::s(&h, "addr"), if alive { GREEN() } else { YELLOW() });
        }
        ui.label(RichText::new("→").size(18.0).color(MUTED()));
        chip(ui, "получатель", &short(&api::s(v, "target")), ACCENT());
    });
    let state = api::s(t, "state");
    ui.horizontal(|ui| {
        theme::pill(ui, &state, state_color(&state));
        ui.label(RichText::new(format!("tunnel {} · работает {} · последний ACK {} назад",
            api::s(t, "tunnel_id"), api::u(t, "uptime_ms").map(ago).unwrap_or_default(),
            api::u(t, "last_ack_age_ms").map(ago).unwrap_or_default())).size(12.0).color(MUTED()))
            .on_hover_text("Зелёный relay — недавно подтвердил доставку (TUNNEL_ACK); жёлтый — подтверждений давно не было \
                            (туннель простаивает, DEGRADED). Отказ обнаружится при отправке, и маршрут будет перестроен.");
    });
}
