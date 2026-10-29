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
}

struct ChatMsg {
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

fn ms(v: Option<u64>) -> String { v.map(|x| format!("{x} мс")).unwrap_or_else(|| "—".into()) }

fn ago(ms_ago: u64) -> String {
    let s = ms_ago / 1000;
    if s < 60 { format!("{s} с") } else if s < 3600 { format!("{} мин", s / 60) } else { format!("{} ч", s / 3600) }
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        theme::apply(&cc.egui_ctx);
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
            local: None, node_log: vec![], log_at: Instant::now(), toasts: vec![],
            t_ping: String::new(), t_lookup: String::new(), t_route: String::new(),
            t_key: String::new(), t_val: String::new(), tool_out: None, route: None,
            c_name: String::new(), c_id: String::new(), alias_in: String::new(), j_filter: 0,
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
        self.s.contact_name(id).map(str::to_owned).unwrap_or_else(|| format!("{}…", short(id)))
    }

    fn connect(&mut self) {
        self.disconnect();
        match self.s.mode {
            // на порту API уже отвечает узел (напр. остался от прошлого запуска) — подключаемся к нему
            Mode::Launch if api::get(self.s.gui_port, "/api/status", &[]).is_ok() => {
                self.toast(format!("На порту {} уже работает узел — подключаюсь к нему", self.s.gui_port), YELLOW);
                self.port.store(self.s.gui_port, Ordering::Relaxed);
            }
            Mode::Launch => match LocalNode::start(&self.s) {
                Ok(n) => {
                    self.toast(format!("Узел запущен (pid {})", n.pid()), GREEN);
                    self.local = Some(n);
                    self.port.store(self.s.gui_port, Ordering::Relaxed);
                }
                Err(e) => self.toast(e, RED),
            },
            Mode::Attach => self.port.store(self.s.attach_port, Ordering::Relaxed),
        }
        let _ = self.s.save();
    }

    fn disconnect(&mut self) {
        self.port.store(0, Ordering::Relaxed);
        if let Some(mut n) = self.local.take() { n.stop(); }
        self.status = None;
        self.conn_err = None;
        self.history.clear();
        self.history_loaded = false;
        self.chats.clear();
    }

    fn drain(&mut self) {
        while let Ok(m) = self.rx.try_recv() {
            match m {
                Msg::Status(Ok(v)) => { self.status = Some(v); self.conn_err = None; }
                Msg::Status(Err(e)) => { if self.port.load(Ordering::Relaxed) != 0 { self.conn_err = Some(e); } }
                Msg::History(h) => { if self.port.load(Ordering::Relaxed) != 0 { self.history = h; self.rebuild_chats(); } }
                Msg::Sent { pending, res } => {
                    self.pending.retain(|p| p.id != pending);
                    if let Err(e) = res { self.toast(format!("Не доставлено: {e}"), RED); }
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
                    Err(e) => self.toast(format!("@{alias}: {e}"), RED),
                },
                Msg::Alias { name, res } => match api::ok(res) {
                    Ok(v) => {
                        self.toast(format!("Псевдоним @{name} опубликован ({} реплик)", api::s(&v, "stored_on")), GREEN);
                        self.s.my_alias = name;
                        let _ = self.s.save();
                    }
                    Err(e) => self.toast(format!("@{name}: {e}"), RED),
                },
            }
        }
    }

    fn rebuild_chats(&mut self) {
        let mut map: BTreeMap<String, Chat> = BTreeMap::new();
        for e in &self.history {
            let peer = api::s(e, "peer");
            if peer.is_empty() { continue; }
            let mine = api::s(e, "dir") == "out";
            let ts = api::u(e, "ts_ms").unwrap_or(0);
            let c = map.entry(peer.clone()).or_insert(Chat { peer, msgs: vec![], last_ts: 0, incoming: 0 });
            c.last_ts = c.last_ts.max(ts);
            if !mine { c.incoming += 1; }
            c.msgs.push(ChatMsg {
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
            if !notified { self.toast(format!("✉ {}: {text}", self.display_name(&peer)), ACCENT); }
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

    fn add_contact(&mut self, name: String, id: String) {
        let name = if name.trim().is_empty() { format!("{}…", short(&id)) } else { name.trim().to_owned() };
        match self.s.contacts.iter_mut().find(|c| c.node_id == id) {
            Some(c) => c.name = name.clone(),
            None => self.s.contacts.push(Contact { name: name.clone(), node_id: id }),
        }
        let _ = self.s.save();
        self.toast(format!("Контакт «{name}» сохранён"), GREEN);
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
            self.toast("Нужен NodeID (64 hex) или @псевдоним", YELLOW);
        }
    }

    fn send(&mut self) {
        let (Some(peer), text) = (self.active.clone(), self.composer.trim().to_owned()) else { return };
        if text.is_empty() { return; }
        self.composer.clear();
        let id = self.next_pending;
        self.next_pending += 1;
        self.pending.push(Pending { id, peer: peer.clone(), text: text.clone(), ts: now_ms() });
        let tx = self.tx.clone();
        self.bg(move |p| {
            let res = api::ok(api::post(p, "/api/send", &[("target", &peer), ("text", &text)]));
            if let Ok(h) = api::get(p, "/api/history", &[("limit", "1000")]) {
                let _ = tx.send(Msg::History(h["entries"].as_array().cloned().unwrap_or_default()));
            }
            Msg::Sent { pending: id, res }
        });
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
                self.toast("Процесс узла завершился — см. журнал на вкладке «Подключение»", RED);
            }
        }
        self.rail(ctx);
        match self.tab {
            Tab::Chats => self.chats_tab(ctx),
            tab => {
                egui::CentralPanel::default()
                    .frame(egui::Frame::default().fill(BG_MAIN).inner_margin(Margin::same(24)))
                    .show(ctx, |ui| {
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
        self.toasts_ui(ctx);
        ctx.request_repaint_after(Duration::from_millis(500));
    }

    /// Закрытие окна: остановить узел, запущенный приложением.
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        if let Some(mut n) = self.local.take() { n.stop(); }
    }
}

impl App {
    fn rail(&mut self, ctx: &egui::Context) {
        egui::SidePanel::left("rail").resizable(false).exact_width(76.0)
            .frame(egui::Frame::default().fill(BG_RAIL).inner_margin(Margin::symmetric(14, 16)))
            .show(ctx, |ui| {
                ui.vertical_centered(|ui| {
                    let (r, _) = ui.allocate_exact_size(Vec2::splat(48.0), egui::Sense::hover());
                    ui.painter().rect_filled(r, CornerRadius::same(16), ACCENT);
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
                        let (c, tip) = if self.connected() { (GREEN, "узел на связи".to_owned()) }
                            else if self.port.load(Ordering::Relaxed) != 0 { (YELLOW, format!("подключение… {}", self.conn_err.clone().unwrap_or_default())) }
                            else { (RED, "не подключено".to_owned()) };
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
            ui.label(RichText::new(self.conn_err.clone().unwrap_or_else(|| "Запустите свой узел или подключитесь к работающему".into())).color(MUTED));
            ui.add_space(8.0);
            if ui.add(primary("Открыть «Подключение»")).clicked() { self.tab = Tab::Connection; }
        });
    }

    // ── Чаты ───────────────────────────────────────────────────────────────

    fn chats_tab(&mut self, ctx: &egui::Context) {
        egui::SidePanel::left("chat_list").resizable(false).exact_width(290.0)
            .frame(egui::Frame::default().fill(BG_SIDE).inner_margin(Margin::symmetric(12, 16)))
            .show(ctx, |ui| {
                ui.label(RichText::new("Чаты").size(20.0).strong());
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
                        ui.label(RichText::new("Пока нет переписки. Введите NodeID собеседника или его @псевдоним выше.").color(MUTED));
                    }
                    for peer in peers {
                        if self.chat_row(ui, &peer) { self.open_chat(peer); }
                    }
                });
            });

        egui::CentralPanel::default().frame(egui::Frame::default().fill(BG_MAIN)).show(ctx, |ui| {
            if !self.connected() { self.not_connected_hint(ui); return; }
            match self.active.clone() {
                None => {
                    ui.add_space(80.0);
                    ui.vertical_centered(|ui| {
                        ui.label(RichText::new("💬").size(64.0));
                        ui.label(RichText::new("Выберите чат или начните новый").size(20.0).strong());
                        ui.label(RichText::new("Сообщения идут через туннель из ≥2 ретрансляторов и шифруются сквозным ключом получателя").color(MUTED));
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
        let fill = if selected { BG_HOVER } else if resp.hovered() { BG_CARD } else { Color32::TRANSPARENT };
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
                            egui::Frame::default().fill(RED).corner_radius(CornerRadius::same(9)).inner_margin(Margin::symmetric(6, 0))
                                .show(ui, |ui| ui.label(RichText::new(n.to_string()).size(12.0).color(Color32::WHITE)));
                        }
                        ui.label(RichText::new(self.time(c.last_ts)).size(12.0).color(MUTED));
                    });
                }
            });
            let preview = chat.and_then(|c| c.msgs.last()).map(|m| {
                if m.mine { format!("Вы: {}", m.text) } else { m.text.clone() }
            }).unwrap_or_else(|| format!("{}…", short(peer)));
            ui.add(egui::Label::new(RichText::new(preview).size(13.0).color(MUTED)).truncate());
        });
        resp.clicked()
    }

    fn conversation(&mut self, ui: &mut egui::Ui, peer: String) {
        let name = self.display_name(&peer);
        egui::TopBottomPanel::top("conv_head")
            .frame(egui::Frame::default().fill(BG_MAIN).inner_margin(Margin::symmetric(20, 12)).stroke(Stroke::new(1.0_f32, BG_RAIL)))
            .show_inside(ui, |ui| {
                ui.horizontal(|ui| {
                    theme::avatar(ui, &peer, &name, 40.0);
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 0.0;
                        ui.label(RichText::new(&name).size(17.0).strong());
                        ui.label(RichText::new(&peer).size(11.5).color(MUTED).monospace());
                    });
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.button("Маршрут").on_hover_text("Цепочка ретрансляторов текущего туннеля").clicked() {
                            self.request_route(peer.clone());
                        }
                        if ui.button("📋").on_hover_text("Скопировать NodeID").clicked() {
                            ui.ctx().copy_text(peer.clone());
                            self.toast("NodeID скопирован", GREEN);
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
            });

        egui::TopBottomPanel::bottom("composer")
            .frame(egui::Frame::default().fill(BG_MAIN).inner_margin(Margin::symmetric(20, 14)))
            .show_inside(ui, |ui| {
                ui.horizontal(|ui| {
                    let r = ui.add(field(&mut self.composer)
                        .hint_text(format!("Сообщение для {name}"))
                        .desired_width(ui.available_width() - 118.0 - ui.spacing().item_spacing.x - 24.0)
                        .margin(Margin::symmetric(12, 10)));
                    let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    if ui.add_sized([118.0, 40.0], primary("Отправить →")).clicked() || enter {
                        self.send();
                        r.request_focus();
                    }
                });
                ui.label(RichText::new("🔒 E2E: X25519 + AES-256-GCM · подпись Ed25519 · доставка подтверждается APP_ACK").size(11.5).color(MUTED));
            });

        egui::CentralPanel::default().frame(egui::Frame::default().fill(BG_MAIN).inner_margin(Margin::symmetric(20, 10)))
            .show_inside(ui, |ui| {
                ScrollArea::vertical().stick_to_bottom(true).auto_shrink([false; 2]).show(ui, |ui| {
                    let chat = self.chats.iter().find(|c| c.peer == peer);
                    if chat.is_none() && !self.pending.iter().any(|p| p.peer == peer) {
                        ui.add_space(40.0);
                        ui.vertical_centered(|ui| ui.label(RichText::new("Сообщений пока нет — напишите первым").color(MUTED)));
                    }
                    if let Some(c) = chat {
                        for m in &c.msgs { self.bubble(ui, m); }
                    }
                    for p in self.pending.iter().filter(|p| p.peer == peer) {
                        let m = ChatMsg { mine: true, text: p.text.clone(), ts: p.ts, status: "pending".into(),
                                          rtt: None, lookup: None, build: None, send: None, error: None };
                        self.bubble(ui, &m);
                    }
                });
            });
    }

    fn bubble(&self, ui: &mut egui::Ui, m: &ChatMsg) {
        let max_w = (ui.available_width() * 0.66).max(240.0);
        let text = if m.text.is_empty() { "—" } else { m.text.as_str() };
        let (icon, icon_color) = match m.status.as_str() {
            "delivered" => ("✔✔", GREEN),
            "sent" => ("✔", TEXT),
            "pending" => ("⏳", YELLOW),
            "received" => ("", TEXT),
            _ => ("✖", RED),
        };
        let footer = format!("{}  {icon}  {}", self.time(m.ts), m.rtt.map(|r| format!("{r} мс")).unwrap_or_default());
        // ширина пузыря = ширина текста (с переносом по max_w), а не всей колонки
        let body = egui::TextStyle::Body.resolve(ui.style());
        let w = ui.fonts(|f| {
            let t = f.layout(text.to_owned(), body.clone(), TEXT, max_w).size().x;
            let foot = f.layout_no_wrap(footer.clone(), egui::FontId::proportional(11.0), TEXT).size().x;
            t.max(foot + 6.0).min(max_w).ceil()
        });
        let (fill, radius) = if m.mine {
            (ACCENT_DIM, CornerRadius { nw: 14, ne: 14, sw: 14, se: 4 })
        } else {
            (BG_CARD, CornerRadius { nw: 14, ne: 14, sw: 4, se: 14 })
        };
        ui.horizontal(|ui| {
            if m.mine { ui.add_space((ui.available_width() - w - 24.0).max(0.0)); }
            let resp = egui::Frame::default().fill(fill).corner_radius(radius).inner_margin(Margin::symmetric(12, 8))
                .show(ui, |ui| {
                    ui.set_width(w);
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 2.0;
                        ui.spacing_mut().interact_size.y = 14.0;
                        ui.add(egui::Label::new(RichText::new(text).color(TEXT)).wrap());
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 6.0;
                            ui.label(RichText::new(self.time(m.ts)).size(11.0).color(TEXT.gamma_multiply(0.6)));
                            if m.mine && !icon.is_empty() {
                                ui.label(RichText::new(icon).size(11.0).color(icon_color));
                                if let Some(r) = m.rtt { ui.label(RichText::new(format!("{r} мс")).size(11.0).color(TEXT.gamma_multiply(0.6))); }
                            }
                        });
                    });
                }).response;
            if m.mine {
                let tip = match m.status.as_str() {
                    "delivered" => format!(
                        "Доставлено и подтверждено получателем (APP_ACK)\nRTT до APP_ACK: {}\nlookup получателя: {}\nтуннель: {}\nпроход по цепочке relay: {}",
                        ms(m.rtt), ms(m.lookup),
                        m.build.map(|b| format!("построен за {b} мс")).unwrap_or_else(|| "уже был установлен".into()),
                        ms(m.send)),
                    "pending" => "Отправляется: lookup → туннель → E2E → ожидание APP_ACK".into(),
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
                    if me.is_empty() { ui.label(RichText::new("узел не подключён").color(MUTED)); }
                    else {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(&me).monospace().size(12.5));
                            if ui.small_button("📋").on_hover_text("Скопировать NodeID").clicked() {
                                ui.ctx().copy_text(me.clone());
                                self.toast("NodeID скопирован — отправьте его собеседнику", GREEN);
                            }
                        });
                        if let Some(st) = &self.status { ui.label(RichText::new(format!("адрес {}", api::s(st, "addr"))).color(MUTED)); }
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
            ui.label(RichText::new("Псевдоним — подписанная запись в DHT: по @имени собеседники найдут ваш NodeID. Занятое имя перезаписать нельзя.").size(12.5).color(MUTED));
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
            ui.label(RichText::new("Пусто. Контакты хранятся локально в настройках приложения.").color(MUTED));
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
                        ui.label(RichText::new(&c.node_id).monospace().size(11.5).color(MUTED));
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
        egui::Frame::default().fill(YELLOW.gamma_multiply(0.08)).stroke(Stroke::new(1.0_f32, YELLOW.gamma_multiply(0.5)))
            .corner_radius(CornerRadius::same(12)).inner_margin(Margin::same(16))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(RichText::new(title).size(16.0).strong().color(YELLOW));
                ui.label(text);
                ui.label(RichText::new(if supported {
                    format!("Узел сообщает features.{feature} = true, но этот клиент ещё не подключён к API функции.")
                } else if self.connected() {
                    format!("Подключённый узел сообщает features.{feature} = false — элементы ниже неактивны.")
                } else {
                    "Узел не подключён.".to_owned()
                }).size(12.5).color(MUTED));
            });
        ui.add_space(14.0);
    }

    fn files_tab(&mut self, ui: &mut egui::Ui) {
        ui.heading("Файлы");
        ui.add_space(10.0);
        self.coming_soon(ui, "files", "🗀 Передача файлов — в разработке",
            "Протокол поверх того же E2E-канала: предложение (имя, размер, SHA-256, число блоков) → блоки ≤ 48 КиБ с номерами → \
             подтверждение окном → итоговая проверка SHA-256. Приём только в выделенный каталог, с лимитом размера.");
        ui.add_enabled_ui(false, |ui| {
            card(ui, |ui| {
                section_title(ui, "Отправить файл");
                ui.horizontal(|ui| {
                    let mut to = String::new();
                    ui.add(field(&mut to).hint_text("получатель: NodeID или @псевдоним").desired_width(320.0));
                    let _ = ui.button("🗀 Выбрать файл…");
                    let _ = ui.add(primary("Отправить"));
                });
                let (r, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 120.0), egui::Sense::hover());
                ui.painter().rect_stroke(r, CornerRadius::same(12), Stroke::new(1.5_f32, MUTED.gamma_multiply(0.5)), egui::StrokeKind::Inside);
                ui.painter().text(r.center(), egui::Align2::CENTER_CENTER, "Перетащите файл сюда",
                                  egui::FontId::proportional(16.0), MUTED);
            });
            ui.add_space(12.0);
            card(ui, |ui| {
                section_title(ui, "Передачи");
                egui::Grid::new("files_grid").num_columns(5).spacing([24.0, 8.0]).show(ui, |ui| {
                    for h in ["Файл", "Собеседник", "Размер", "Прогресс", "Статус"] { ui.label(RichText::new(h).strong()); }
                    ui.end_row();
                    ui.label(RichText::new("нет передач").color(MUTED));
                    ui.end_row();
                });
            });
        });
    }

    fn calls_tab(&mut self, ui: &mut egui::Ui) {
        ui.heading("Звонки");
        ui.add_space(10.0);
        self.coming_soon(ui, "calls", "📞 Голосовые звонки — запланированы",
            "Групповой звонок 3–5 узлов в LAN: состав звонка — подписанная DHT-запись, сигнализация INVITE / ACCEPT / BYE \
             по защищённому каналу, ключ звонка передаётся каждому участнику E2E, инициатор — временный SFU.");
        ui.add_enabled_ui(false, |ui| {
            card(ui, |ui| {
                section_title(ui, "Участники");
                let people: Vec<(String, String)> = self.s.contacts.iter().take(4)
                    .map(|c| (c.node_id.clone(), c.name.clone())).collect();
                ui.horizontal_wrapped(|ui| {
                    let me = self.my_id();
                    let mut tiles = vec![(if me.is_empty() { "me".into() } else { me }, "Вы".to_owned())];
                    tiles.extend(people);
                    for (id, name) in tiles {
                        egui::Frame::default().fill(BG_MAIN).corner_radius(CornerRadius::same(14)).inner_margin(Margin::same(16))
                            .show(ui, |ui| {
                                ui.set_width(150.0);
                                ui.vertical_centered(|ui| {
                                    theme::avatar(ui, &id, &name, 64.0);
                                    ui.label(RichText::new(&name).strong());
                                    ui.label(RichText::new("🔇 не в звонке").size(12.0).color(MUTED));
                                });
                            });
                    }
                });
            });
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                let _ = ui.add_sized([170.0, 44.0], primary("📞 Начать звонок"));
                let _ = ui.add_sized([140.0, 44.0], egui::Button::new("🔇 Микрофон"));
                let _ = ui.add_sized([140.0, 44.0], egui::Button::new("🔊 Звук"));
                let _ = ui.add_sized([150.0, 44.0], danger("⛔ Завершить"));
            });
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
                egui::Frame::default().fill(BG_CARD).corner_radius(CornerRadius::same(12)).inner_margin(Margin::same(14))
                    .show(ui, |ui| {
                        ui.vertical(|ui| {
                            ui.set_min_width(120.0);
                            ui.label(RichText::new(v).size(26.0).strong());
                            ui.label(RichText::new(l).color(MUTED));
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
                    Ok(v) if v.get("ok").and_then(Value::as_bool) != Some(false) => (GREEN, serde_json::to_string_pretty(v).unwrap_or_default()),
                    Ok(v) => (RED, serde_json::to_string_pretty(v).unwrap_or_default()),
                    Err(e) => (RED, e.clone()),
                };
                ui.label(RichText::new(title).color(c).strong());
                egui::Frame::default().fill(BG_INPUT).corner_radius(CornerRadius::same(8)).inner_margin(Margin::same(10)).show(ui, |ui| {
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
            if tunnels.is_empty() { ui.label(RichText::new("нет — туннель строится при первой отправке").color(MUTED)); }
            egui::Grid::new("tunnels").num_columns(6).striped(true).spacing([24.0, 6.0]).show(ui, |ui| {
                if !tunnels.is_empty() {
                    for h in ["tunnel_id", "состояние", "hops", "живых", "посл. ACK", "uptime"] { ui.label(RichText::new(h).strong()); }
                    ui.end_row();
                }
                for t in &tunnels {
                    ui.label(RichText::new(api::s(t, "tunnel_id")).monospace());
                    let state = api::s(t, "state");
                    theme::pill(ui, &state, state_color(&state));
                    ui.label(api::s(t, "hops"));
                    ui.label(api::s(t, "alive_hops"));
                    ui.label(api::u(t, "last_ack_age_ms").map(|x| format!("{} назад", ago(x))).unwrap_or("—".into()));
                    ui.label(api::u(t, "uptime_ms").map(ago).unwrap_or_default());
                    ui.end_row();
                }
            });
        });
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
        ui.label(RichText::new("Локальная история узла (state/history.jsonl): переживает перезапуск.").color(MUTED));
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
                    let c = match status.as_str() { "delivered" | "received" => GREEN, "failed" => RED, _ => YELLOW };
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
                            ui.label(RichText::new(format!("будет использовано: {}", find_node_binary(&self.s.node_bin).display())).size(12.0).color(MUTED));
                        });
                        ui.end_row();
                        ui.label("Порт узла (P2P)"); ui.add(egui::DragValue::new(&mut self.s.listen_port).range(1024..=65535)); ui.end_row();
                        ui.label("Порт API (127.0.0.1)"); ui.add(egui::DragValue::new(&mut self.s.gui_port).range(1024..=65535)); ui.end_row();
                        ui.label("Bootstrap-пиры");
                        ui.vertical(|ui| {
                            ui.add(field(&mut self.s.bootstrap).hint_text("127.0.0.1:7001, …").desired_width(420.0));
                            ui.label(RichText::new("пусто — узел станет первым (seed) узлом сети").size(12.0).color(MUTED));
                        });
                        ui.end_row();
                        ui.label("Каталог состояния");
                        ui.vertical(|ui| {
                            ui.add(field(&mut self.s.state_dir).desired_width(420.0));
                            ui.label(RichText::new("identity.key (= ваш NodeID), history.jsonl, журнал узла").size(12.0).color(MUTED));
                        });
                        ui.end_row();
                        ui.label("Автозапуск"); ui.checkbox(&mut self.s.autostart, "запускать узел при старте приложения"); ui.end_row();
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
                    match self.s.save() { Ok(()) => self.toast("Настройки сохранены", GREEN), Err(e) => self.toast(e, RED) }
                }
            });
            ui.add_space(4.0);
            let line = if self.connected() { (GREEN, format!("на связи: {}", self.my_id())) }
                else if let Some(e) = &self.conn_err { (YELLOW, e.clone()) }
                else { (MUTED, "не подключено".into()) };
            ui.label(RichText::new(line.1).color(line.0).size(12.5));
        });

        if let Some(n) = &self.local {
            if self.log_at.elapsed() > Duration::from_millis(1500) {
                self.node_log = n.log_tail(300);
                self.log_at = Instant::now();
            }
            ui.add_space(12.0);
            card(ui, |ui| {
                section_title(ui, &format!("Журнал узла · pid {}", n.pid()));
                ui.label(RichText::new(format!("{}  →  {}", n.bin.display(), n.log_path.display())).size(12.0).color(MUTED));
                egui::Frame::default().fill(BG_INPUT).corner_radius(CornerRadius::same(8)).inner_margin(Margin::same(10)).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ScrollArea::vertical().id_salt("node_log").max_height(320.0).stick_to_bottom(true).show(ui, |ui| {
                        for l in &self.node_log {
                            let c = if l.contains(" WARN ") { YELLOW } else if l.contains(" ERROR ") { RED } else { MUTED };
                            ui.label(RichText::new(l).monospace().size(11.5).color(c));
                        }
                    });
                });
            });
        }
    }

    fn toasts_ui(&mut self, ctx: &egui::Context) {
        self.toasts.retain(|t| t.at.elapsed() < Duration::from_secs(5));
        if self.toasts.is_empty() { return; }
        egui::Area::new(egui::Id::new("toasts")).anchor(egui::Align2::RIGHT_BOTTOM, Vec2::new(-18.0, -18.0))
            .show(ctx, |ui| {
                for t in self.toasts.iter().rev().take(4) {
                    egui::Frame::default().fill(BG_SIDE).stroke(Stroke::new(1.0_f32, t.color))
                        .corner_radius(CornerRadius::same(10)).inner_margin(Margin::symmetric(14, 10))
                        .show(ui, |ui| { ui.set_max_width(380.0); ui.label(RichText::new(&t.text).color(TEXT)); });
                    ui.add_space(6.0);
                }
            });
    }
}

/// Иконка вкладки на левой панели: подсветка выбранной, счётчик непрочитанного, метка «скоро».
fn rail_button(ui: &mut egui::Ui, icon: &str, selected: bool, badge: usize, soon: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(48.0), egui::Sense::click());
    let p = ui.painter();
    let fill = if selected { ACCENT } else if resp.hovered() { BG_HOVER } else { BG_SIDE };
    p.rect_filled(rect, CornerRadius::same(if selected || resp.hovered() { 16 } else { 24 }), fill);
    if selected {
        let bar = egui::Rect::from_min_size(egui::pos2(rect.left() - 14.0, rect.top() + 8.0), Vec2::new(4.0, 32.0));
        p.rect_filled(bar, CornerRadius::same(2), Color32::WHITE);
    }
    let c = if soon && !selected { MUTED } else { Color32::WHITE };
    p.text(rect.center(), egui::Align2::CENTER_CENTER, icon, egui::FontId::proportional(20.0), c);
    if badge > 0 {
        let center = rect.right_bottom() - Vec2::new(6.0, 6.0);
        p.circle_filled(center, 9.0, RED);
        p.text(center, egui::Align2::CENTER_CENTER, badge.min(99).to_string(), egui::FontId::proportional(11.0), Color32::WHITE);
    } else if soon {
        let center = rect.right_top() + Vec2::new(-6.0, 6.0);
        p.circle_filled(center, 5.0, YELLOW);
    }
    resp
}

/// Цепочка «Вы → relay → relay → получатель» по ответу `/api/route`.
fn route_chain(ui: &mut egui::Ui, r: &Result<Value, String>) {
    let v = match r { Ok(v) => v, Err(e) => { ui.label(RichText::new(e).color(RED)); return; } };
    let t = &v["tunnel"];
    if t.is_null() {
        ui.label(RichText::new("Туннеля сейчас нет — он будет построен при следующей отправке.").color(MUTED));
        return;
    }
    let chip = |ui: &mut egui::Ui, title: &str, sub: &str, c: Color32| {
        egui::Frame::default().fill(c.gamma_multiply(0.12)).stroke(Stroke::new(1.0_f32, c.gamma_multiply(0.7)))
            .corner_radius(CornerRadius::same(10)).inner_margin(Margin::symmetric(10, 6))
            .show(ui, |ui| {
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    ui.label(RichText::new(title).strong().color(c));
                    if !sub.is_empty() { ui.label(RichText::new(sub).size(11.0).color(MUTED)); }
                });
            });
    };
    ui.horizontal_wrapped(|ui| {
        chip(ui, "Вы", "инициатор", GREEN);
        for h in t["hops"].as_array().cloned().unwrap_or_default() {
            ui.label(RichText::new("→").size(18.0).color(MUTED));
            let alive = h["alive"].as_bool().unwrap_or(false);
            chip(ui, &format!("relay {}", short(&api::s(&h, "node_id"))), &api::s(&h, "addr"), if alive { GREEN } else { YELLOW });
        }
        ui.label(RichText::new("→").size(18.0).color(MUTED));
        chip(ui, "получатель", &short(&api::s(v, "target")), ACCENT);
    });
    let state = api::s(t, "state");
    ui.horizontal(|ui| {
        theme::pill(ui, &state, state_color(&state));
        ui.label(RichText::new(format!("tunnel {} · работает {} · последний ACK {} назад",
            api::s(t, "tunnel_id"), api::u(t, "uptime_ms").map(ago).unwrap_or_default(),
            api::u(t, "last_ack_age_ms").map(ago).unwrap_or_default())).size(12.0).color(MUTED))
            .on_hover_text("Зелёный relay — недавно подтвердил доставку (TUNNEL_ACK); жёлтый — подтверждений давно не было \
                            (туннель простаивает, DEGRADED). Отказ обнаружится при отправке, и маршрут будет перестроен.");
    });
}
