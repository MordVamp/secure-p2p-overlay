//! theme.rs — темы, шрифты, оформление и мелкие виджеты (аватар, карточка, «таблетка»).
//!
//! Темы: тёмная, светлая, две вампирские («Кровавая луна», «Ночь в замке») и две фейские («Лесная фея», «Цветочная
//! фея»). Цвета — функции текущей палитры (`ACCENT()` и т. п.), смена темы перерисовывает всё. У декоративных тем —
//! оформление, а не только цвет (`decor_top`, `decor_bg`, `bubble_decor`): стекающая кровь, луна, замок, летучие мыши,
//! светлячки и мухоморы, цветы, бабочки и пыльца — всё рисуется кистью egui, без картинок.
#![allow(non_snake_case)]

use std::f32::consts::{PI, TAU};
use std::sync::atomic::{AtomicU8, Ordering};

use eframe::egui::{self, pos2, vec2, Color32, CornerRadius, FontFamily, FontId, Margin, Pos2, Rect, RichText, Sense, Shape,
                   Stroke, TextStyle, Vec2};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Theme { Dark, Light, Blood, Night, Forest, Bloom }

pub const THEMES: [(Theme, &str, &str); 6] = [
    (Theme::Dark, "dark", "Тёмная"),
    (Theme::Light, "light", "Светлая"),
    (Theme::Blood, "blood", "Кровавая луна"),
    (Theme::Night, "night", "Ночь в замке"),
    (Theme::Forest, "forest", "Лесная фея"),
    (Theme::Bloom, "bloom", "Цветочная фея"),
];

struct Pal {
    rail: u32, side: u32, main: u32, card: u32, input: u32, hover: u32, button: u32, button_hover: u32,
    accent: u32, accent_dim: u32, green: u32, red: u32, yellow: u32, text: u32, muted: u32,
    dark: bool, avatars: [u32; 8],
}

const PALS: [Pal; 6] = [
    Pal { rail: 0x1a1b1f, side: 0x24262b, main: 0x2c2e34, card: 0x36383f, input: 0x1f2024, hover: 0x3d4048, button: 0x474a53,
          button_hover: 0x525660, accent: 0x6c7bff, accent_dim: 0x4a55c9, green: 0x3bc47a, red: 0xf0565a, yellow: 0xf2b83c,
          text: 0xe3e5e8, muted: 0x9aa0aa, dark: true,
          avatars: [0xef6c6c, 0xf09a4a, 0xd9b33c, 0x52b788, 0x3fa7c9, 0x6c7bff, 0xa86be0, 0xe06bb4] },
    Pal { rail: 0xf3f4f7, side: 0xffffff, main: 0xe9ebf0, card: 0xffffff, input: 0xe6e8ed, hover: 0xdde0e7, button: 0xd5d9e1,
          button_hover: 0xc9ced8, accent: 0x4a55c9, accent_dim: 0x5c68e6, green: 0x1b9a55, red: 0xd3383d, yellow: 0xb7791f,
          text: 0x1c1e23, muted: 0x5e6571, dark: false,
          avatars: [0xef6c6c, 0xf09a4a, 0xd9b33c, 0x52b788, 0x3fa7c9, 0x6c7bff, 0xa86be0, 0xe06bb4] },
    Pal { rail: 0x0e0405, side: 0x1c0809, main: 0x140607, card: 0x2b0e11, input: 0x0a0203, hover: 0x3a1418, button: 0x4a1a1f,
          button_hover: 0x5e2228, accent: 0xd3162a, accent_dim: 0x8e0a17, green: 0x57b87a, red: 0xff4040, yellow: 0xe59a3a,
          text: 0xf3e4e4, muted: 0xa8817f, dark: true,
          avatars: [0xb3121f, 0x7a0e16, 0xd9483b, 0x8c5a3c, 0x9e2b4f, 0x5e1f2e, 0xc0392b, 0x6b4e4e] },
    Pal { rail: 0x0b0912, side: 0x16121f, main: 0x100d18, card: 0x221b2e, input: 0x0a0810, hover: 0x2e2540, button: 0x3a3050,
          button_hover: 0x4a3d66, accent: 0xb43a5e, accent_dim: 0x6e2240, green: 0x6fc7a0, red: 0xf0566e, yellow: 0xe8d28c,
          text: 0xe9e5f0, muted: 0x9c94ae, dark: true,
          avatars: [0x6e2240, 0x4b3a78, 0x7a5ba6, 0x3d5a7a, 0x8c3b5e, 0x5a6b8c, 0xa0607f, 0x4e4566] },
    Pal { rail: 0x0a1710, side: 0x10241a, main: 0x0d1d14, card: 0x1a3325, input: 0x07120c, hover: 0x24432f, button: 0x2c503a,
          button_hover: 0x386348, accent: 0x9be564, accent_dim: 0x3e8e5a, green: 0x7be0a0, red: 0xf07a7a, yellow: 0xf4d35e,
          text: 0xe6f4e8, muted: 0x93b39c, dark: true,
          avatars: [0x3e8e5a, 0x6b8e23, 0xb5651d, 0x2e7d6b, 0x8a9a3c, 0xc0504d, 0x5c7a3a, 0xa0784a] },
    Pal { rail: 0xf6ecf7, side: 0xfff8fd, main: 0xfbf2f9, card: 0xffffff, input: 0xf3e6f2, hover: 0xf0ddf0, button: 0xe9d3ec,
          button_hover: 0xdec3e3, accent: 0xb05fcb, accent_dim: 0xc77dd9, green: 0x4caf7a, red: 0xe0567a, yellow: 0xd9a21b,
          text: 0x3a2840, muted: 0x8a7490, dark: false,
          avatars: [0xe58fb1, 0xb07fd8, 0x7fb3e5, 0x8fd1a8, 0xf0b36b, 0xd98fd1, 0x9a8fe5, 0xe5a38f] },
];

static CUR: AtomicU8 = AtomicU8::new(0);

pub fn current() -> Theme { THEMES[CUR.load(Ordering::Relaxed) as usize % THEMES.len()].0 }
fn pal() -> &'static Pal { &PALS[CUR.load(Ordering::Relaxed) as usize % PALS.len()] }
fn c(rgb: u32) -> Color32 { Color32::from_rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8) }

pub fn BG_RAIL() -> Color32 { c(pal().rail) }
pub fn BG_SIDE() -> Color32 { c(pal().side) }
pub fn BG_MAIN() -> Color32 { c(pal().main) }
pub fn BG_CARD() -> Color32 { c(pal().card) }
pub fn BG_INPUT() -> Color32 { c(pal().input) }
pub fn BG_HOVER() -> Color32 { c(pal().hover) }
/// Фон кнопок/переключателей: светлее карточки, чтобы не сливаться с ней.
pub fn BG_BUTTON() -> Color32 { c(pal().button) }
pub fn ACCENT() -> Color32 { c(pal().accent) }
pub fn ACCENT_DIM() -> Color32 { c(pal().accent_dim) }
pub fn GREEN() -> Color32 { c(pal().green) }
pub fn RED() -> Color32 { c(pal().red) }
pub fn YELLOW() -> Color32 { c(pal().yellow) }
pub fn TEXT() -> Color32 { c(pal().text) }
pub fn MUTED() -> Color32 { c(pal().muted) }
/// Текст на акцентной заливке (свои сообщения, основные кнопки).
pub fn ON_ACCENT() -> Color32 { Color32::WHITE }

/// Декоративная тема: нужна анимация (перерисовка ~25 раз в секунду).
pub fn animated() -> bool { !matches!(current(), Theme::Dark | Theme::Light) }

/// Значок темы у заголовка (рисуется кистью: в системном шрифте может не быть эмодзи).
pub fn glyph(ui: &mut egui::Ui, size: f32) {
    if !animated() { return; }
    let (r, _) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    let p = ui.painter_at(r.expand(2.0));
    let c0 = r.center();
    let t = ui.input(|i| i.time) as f32;
    match current() {
        Theme::Blood => drip(&p, pos2(c0.x, r.min.y + 2.0), size * 0.55, size * 0.85),
        Theme::Night => bat(&p, c0, size * 0.5, (t * 6.0).sin() * 0.5 + 0.5, MUTED()),
        Theme::Forest => mushroom(&p, pos2(c0.x, r.max.y), size * 0.95),
        Theme::Bloom => flower(&p, c0, size * 0.45, Color32::from_rgb(0xe0, 0x7f, 0xb4)),
        _ => {}
    }
}

/// Шрифты: системный гротеск для текста; антиква для заголовков декоративных тем (если есть).
fn install_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    let candidates = [
        "/usr/share/fonts/Adwaita/AdwaitaSans-Regular.ttf",
        "/usr/share/fonts/inter/Inter-Regular.ttf",
        "/usr/share/fonts/TTF/Inter-Regular.ttf",
        "/usr/share/fonts/noto/NotoSans-Regular.ttf",
        "/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf",
        "/usr/share/fonts/TTF/DejaVuSans.ttf",
        "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
    ];
    if let Some(bytes) = candidates.iter().find_map(|p| std::fs::read(p).ok()) {
        fonts.font_data.insert("ui".into(), std::sync::Arc::new(egui::FontData::from_owned(bytes)));
        fonts.families.entry(FontFamily::Proportional).or_default().insert(0, "ui".into());
    }
    let serif = [
        "/usr/share/fonts/noto/NotoSerif-Regular.ttf",
        "/usr/share/fonts/truetype/noto/NotoSerif-Regular.ttf",
        "/usr/share/fonts/TTF/DejaVuSerif.ttf",
        "/usr/share/fonts/truetype/dejavu/DejaVuSerif.ttf",
        "/usr/share/fonts/gnu-free/FreeSerif.otf",
        "/usr/share/fonts/truetype/freefont/FreeSerif.ttf",
    ];
    let mut fam = fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    if let Some(bytes) = serif.iter().find_map(|p| std::fs::read(p).ok()) {
        fonts.font_data.insert("serif".into(), std::sync::Arc::new(egui::FontData::from_owned(bytes)));
        fam.insert(0, "serif".into());
    }
    fonts.families.insert(FontFamily::Name("serif".into()), fam);
    ctx.set_fonts(fonts);
}

/// Применить тему по идентификатору из настроек (`dark`, `light`, `blood`, `night`, `forest`, `bloom`).
pub fn set(ctx: &egui::Context, id: &str) {
    let i = THEMES.iter().position(|t| t.1 == id).unwrap_or(0);
    CUR.store(i as u8, Ordering::Relaxed);
    visuals(ctx);
}

pub fn apply(ctx: &egui::Context) {
    install_fonts(ctx);
    visuals(ctx);
}

fn visuals(ctx: &egui::Context) {
    let p = pal();
    let mut v = if p.dark { egui::Visuals::dark() } else { egui::Visuals::light() };
    v.panel_fill = BG_MAIN();
    v.window_fill = BG_SIDE();
    v.extreme_bg_color = BG_INPUT();
    v.faint_bg_color = BG_CARD().lerp_to_gamma(BG_MAIN(), 0.5);
    v.override_text_color = Some(TEXT());
    v.hyperlink_color = ACCENT();
    v.selection.bg_fill = ACCENT_DIM();
    v.selection.stroke = Stroke::new(1.0_f32, ON_ACCENT());
    v.window_stroke = Stroke::new(1.0_f32, BG_HOVER());
    let r = CornerRadius::same(8);
    for (w, fill) in [
        (&mut v.widgets.noninteractive, BG_CARD()),
        (&mut v.widgets.inactive, BG_BUTTON()),
        (&mut v.widgets.hovered, c(p.button_hover)),
        (&mut v.widgets.active, ACCENT_DIM()),
        (&mut v.widgets.open, BG_HOVER()),
    ] {
        w.corner_radius = r;
        w.weak_bg_fill = fill;
        w.bg_fill = fill;
    }
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, BG_HOVER());
    v.widgets.inactive.bg_stroke = Stroke::NONE;
    v.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, ACCENT());
    v.widgets.inactive.fg_stroke = Stroke::new(1.0_f32, TEXT());
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, TEXT());
    ctx.set_visuals(v);
    let head = if animated() { FontFamily::Name("serif".into()) } else { FontFamily::Proportional };
    ctx.style_mut(|s| {
        s.spacing.item_spacing = Vec2::new(8.0, 8.0);
        s.spacing.button_padding = Vec2::new(14.0, 7.0);
        s.spacing.interact_size.y = 32.0;
        s.text_styles.insert(TextStyle::Body, FontId::proportional(15.0));
        s.text_styles.insert(TextStyle::Button, FontId::proportional(15.0));
        s.text_styles.insert(TextStyle::Heading, FontId::new(23.0, head));
        s.text_styles.insert(TextStyle::Small, FontId::proportional(12.0));
        s.text_styles.insert(TextStyle::Monospace, FontId::monospace(13.0));
    });
}

pub fn color_for(id: &str) -> Color32 {
    let h = id.bytes().fold(5381u32, |h, b| h.wrapping_mul(33) ^ b as u32);
    let a = &pal().avatars;
    c(a[(h as usize) % a.len()])
}

/// Круглый аватар с инициалами; цвет стабилен для NodeID.
pub fn avatar(ui: &mut egui::Ui, key: &str, label: &str, size: f32) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size), Sense::click());
    let p = ui.painter();
    p.circle_filled(rect.center(), size / 2.0, color_for(key));
    // инициалы: первые буквы двух слов, иначе одна буква
    let words: Vec<&str> = label.trim_start_matches('@').split_whitespace().collect();
    let initials: String = words.iter().take(2)
        .filter_map(|w| w.chars().find(|c| c.is_alphanumeric()))
        .collect::<String>().to_uppercase();
    p.text(rect.center(), egui::Align2::CENTER_CENTER, initials, FontId::proportional(size * 0.4), Color32::WHITE);
    resp
}

pub fn card<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> egui::InnerResponse<R> {
    egui::Frame::default()
        .fill(BG_CARD())
        .corner_radius(CornerRadius::same(12))
        .inner_margin(Margin::same(16))
        .show(ui, |ui| { ui.set_width(ui.available_width()); add(ui) })
}

/// Цветная «таблетка» со статусом.
pub fn pill(ui: &mut egui::Ui, text: &str, color: Color32) { pill_resp(ui, text, color); }

/// Однострочное поле ввода с внутренними отступами.
pub fn field(s: &mut String) -> egui::TextEdit<'_> {
    egui::TextEdit::singleline(s).margin(Margin::symmetric(10, 7))
}

/// То же, что `pill`, но возвращает Response (для подсказки).
pub fn pill_resp(ui: &mut egui::Ui, text: &str, color: Color32) -> egui::Response {
    egui::Frame::default()
        .fill(color.gamma_multiply(0.18))
        .stroke(Stroke::new(1.0_f32, color.gamma_multiply(0.6)))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::symmetric(8, 2))
        .show(ui, |ui| { ui.label(RichText::new(text).color(color).size(12.5)); })
        .response
}

pub fn section_title(ui: &mut egui::Ui, text: &str) {
    ui.label(RichText::new(text.to_uppercase()).size(12.0).color(MUTED()).strong());
}

pub fn primary(text: &str) -> egui::Button<'static> {
    egui::Button::new(RichText::new(text.to_owned()).color(ON_ACCENT())).fill(ACCENT_DIM())
}

pub fn danger(text: &str) -> egui::Button<'static> {
    egui::Button::new(RichText::new(text.to_owned()).color(Color32::WHITE)).fill(RED().gamma_multiply(0.75))
}

pub fn state_color(state: &str) -> Color32 {
    match state {
        "Active" => GREEN(),
        "Degraded" | "Building" => YELLOW(),
        _ => RED(),
    }
}

// ── выбор темы ───────────────────────────────────────────────────────────────

/// Карточки тем с миниатюрой. Возвращает выбранный идентификатор, если сменили.
pub fn picker(ui: &mut egui::Ui) -> Option<&'static str> {
    let mut picked = None;
    let cur = CUR.load(Ordering::Relaxed) as usize;
    ui.horizontal_wrapped(|ui| {
        for (i, (_, id, title)) in THEMES.iter().enumerate() {
            let (rect, resp) = ui.allocate_exact_size(vec2(132.0, 104.0), Sense::click());
            let p = ui.painter_at(rect);
            let pl = &PALS[i];
            let stroke = if i == cur { Stroke::new(2.5_f32, ACCENT()) } else if resp.hovered() { Stroke::new(1.5_f32, MUTED()) }
                         else { Stroke::new(1.0_f32, MUTED().gamma_multiply(0.4)) };
            let img = Rect::from_min_size(rect.min + vec2(6.0, 6.0), vec2(rect.width() - 12.0, 64.0));
            p.rect_filled(img, CornerRadius::same(8), c(pl.main));
            p.rect_filled(Rect::from_min_size(img.min, vec2(img.width(), 12.0)), CornerRadius { nw: 8, ne: 8, sw: 0, se: 0 }, c(pl.side));
            p.rect_filled(Rect::from_min_size(img.min + vec2(8.0, 22.0), vec2(56.0, 12.0)), CornerRadius::same(5), c(pl.card));
            p.rect_filled(Rect::from_min_size(img.min + vec2(img.width() - 70.0, 42.0), vec2(60.0, 12.0)), CornerRadius::same(5), c(pl.accent_dim));
            let t = ui.input(|i| i.time) as f32;
            thumb(&p, img, THEMES[i].0, t);
            p.rect_stroke(rect.shrink(1.0), CornerRadius::same(10), stroke, egui::StrokeKind::Inside);
            p.text(pos2(rect.center().x, rect.max.y - 18.0), egui::Align2::CENTER_CENTER, *title,
                   FontId::proportional(13.0), if i == cur { TEXT() } else { MUTED() });
            if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() && i != cur { picked = Some(*id); }
        }
    });
    picked
}

fn thumb(p: &egui::Painter, r: Rect, th: Theme, t: f32) {
    match th {
        Theme::Blood => {
            p.rect_filled(Rect::from_min_size(r.min, vec2(r.width(), 6.0)), CornerRadius::same(2), BLOOD_DARK);
            for (x, l) in [(0.15, 14.0), (0.4, 22.0), (0.7, 10.0), (0.88, 18.0)] { drip(p, pos2(r.min.x + r.width() * x, r.min.y + 5.0), 5.0, l); }
        }
        Theme::Night => {
            p.circle_filled(pos2(r.max.x - 22.0, r.min.y + 24.0), 11.0, MOON);
            bat(p, pos2(r.min.x + 40.0, r.min.y + 26.0), 9.0, (t * 2.0).sin() * 0.5 + 0.5, Color32::from_rgb(7, 5, 12));
        }
        Theme::Forest => {
            mushroom(p, pos2(r.min.x + 24.0, r.max.y - 4.0), 12.0);
            for k in 0..4 { firefly(p, pos2(r.min.x + 50.0 + k as f32 * 14.0, r.min.y + 26.0 + (t + k as f32).sin() * 6.0), 1.0); }
        }
        Theme::Bloom => {
            flower(p, pos2(r.min.x + 22.0, r.max.y - 12.0), 8.0, Color32::from_rgb(0xf2, 0x9b, 0xc4));
            butterfly(p, pos2(r.max.x - 30.0, r.min.y + 28.0), 10.0, (t * 3.0).sin() * 0.5 + 0.5, Color32::from_rgb(0xb0, 0x7f, 0xd8));
            sparkle(p, pos2(r.min.x + 60.0, r.min.y + 30.0), 4.0, Color32::from_rgb(0xff, 0xd7, 0x6b));
        }
        _ => {}
    }
}

// ── оформление ───────────────────────────────────────────────────────────────

const BLOOD: Color32 = Color32::from_rgb(0xb0, 0x10, 0x1e);
const BLOOD_DARK: Color32 = Color32::from_rgb(0x5e, 0x04, 0x0b);
const MOON: Color32 = Color32::from_rgb(0xed, 0xe6, 0xc8);

/// Детерминированный «случайный» ряд: оформление не прыгает между кадрами.
fn rnd(i: u32, salt: u32) -> f32 {
    let x = ((i as f32) * 12.9898 + salt as f32 * 78.233).sin() * 43758.547;
    x - x.floor()
}

/// Верхний край окна: кровь стекает (капли срываются), готические арки, лиана с листьями, гирлянда цветов.
pub fn decor_top(ctx: &egui::Context) {
    if !animated() { return; }
    let screen = ctx.screen_rect();
    let p = ctx.layer_painter(egui::LayerId::new(egui::Order::Middle, egui::Id::new("decor_top")));
    let t = ctx.input(|i| i.time) as f32;
    let w = screen.width();
    match current() {
        Theme::Blood => {
            let top = 5.0;
            let mut edge = vec![pos2(0.0, 0.0)];
            let mut x = 0.0;
            let mut i = 0;
            while x <= w + 10.0 { edge.push(pos2(x, top + rnd(i, 1) * 3.0)); x += 12.0; i += 1; }
            edge.push(pos2(w, 0.0));
            p.add(Shape::convex_polygon(vec![pos2(0.0, 0.0), pos2(w, 0.0), pos2(w, top), pos2(0.0, top)], BLOOD_DARK, Stroke::NONE));
            p.add(Shape::line(edge, Stroke::new(3.0_f32, BLOOD)));
            let mut x = 10.0;
            let mut i = 0;
            while x < w {
                let len = 4.0 + rnd(i, 3) * rnd(i, 5) * 26.0;
                drip(&p, pos2(x, top), 5.0 + rnd(i, 2) * 4.0, len);
                if rnd(i, 7) > 0.8 {
                    // капля набухает и срывается
                    let period = 3.0 + rnd(i, 8) * 3.0;
                    let ph = ((t + rnd(i, 9) * period) % period) / period;
                    let tip = pos2(x, top + len);
                    if ph < 0.55 { p.circle_filled(tip, 1.5 + ph * 2.5, BLOOD); }
                    else {
                        let q = (ph - 0.55) / 0.45;
                        p.circle_filled(pos2(x, tip.y + q * q * 90.0), 2.6, BLOOD.gamma_multiply(1.0 - q * 0.8));
                    }
                }
                x += 22.0 + rnd(i, 4) * 46.0;
                i += 1;
            }
        }
        Theme::Night => {
            let aw = 22.0;
            let mut x = 0.0;
            while x < w {
                let pts: Vec<Pos2> = (0..=10).map(|k| {
                    let u = k as f32 / 10.0;
                    pos2(x + u * aw, 1.0 + (u * PI).sin().powf(0.6) * 9.0)
                }).collect();
                p.add(Shape::line(pts, Stroke::new(1.2_f32, Color32::from_rgb(0xb9, 0xb4, 0xc8).gamma_multiply(0.45))));
                x += aw;
            }
        }
        Theme::Forest => {
            // лиана: волна с листьями и светящимися ягодами
            let pts: Vec<Pos2> = (0..=((w / 6.0) as i32)).map(|k| { let x = k as f32 * 6.0; pos2(x, 7.0 + (x / 38.0).sin() * 4.0) }).collect();
            p.add(Shape::line(pts, Stroke::new(2.0_f32, Color32::from_rgb(0x2f, 0x5e, 0x3a))));
            let mut x = 14.0;
            let mut i = 0;
            while x < w {
                let y = 7.0 + (x / 38.0).sin() * 4.0;
                let dir = if i % 2 == 0 { 1.0 } else { -1.0 };
                leaf(&p, pos2(x, y), 9.0 + rnd(i, 3) * 5.0, 1.2 * dir + 0.3 * (t * 0.8 + i as f32).sin(), Color32::from_rgb(0x4f, 0x9a, 0x4a));
                if rnd(i, 5) > 0.7 {
                    let glow = 0.6 + 0.4 * (t * 2.0 + i as f32).sin();
                    p.circle_filled(pos2(x + 5.0, y + 7.0), 5.0, Color32::from_rgb(0xf4, 0xd3, 0x5e).gamma_multiply(0.18 * glow));
                    p.circle_filled(pos2(x + 5.0, y + 7.0), 2.2, Color32::from_rgb(0xf4, 0xd3, 0x5e).gamma_multiply(glow));
                }
                x += 26.0 + rnd(i, 4) * 20.0;
                i += 1;
            }
        }
        Theme::Bloom => {
            // гирлянда: провисающая нить с цветами
            let mut x0 = 0.0;
            let span = 160.0;
            while x0 < w {
                let pts: Vec<Pos2> = (0..=16).map(|k| { let u = k as f32 / 16.0; pos2(x0 + u * span, 2.0 + (u * PI).sin() * 10.0) }).collect();
                p.add(Shape::line(pts, Stroke::new(1.2_f32, Color32::from_rgb(0x8f, 0xc9, 0x9b))));
                for k in [0.2f32, 0.5, 0.8] {
                    let i = ((x0 / span) as u32) * 3 + (k * 10.0) as u32;
                    let col = [Color32::from_rgb(0xf2, 0x9b, 0xc4), Color32::from_rgb(0xc9, 0x9b, 0xf2), Color32::from_rgb(0xff, 0xc8, 0x7a)][(i % 3) as usize];
                    flower(&p, pos2(x0 + k * span, 2.0 + (k * PI).sin() * 10.0 + 2.0), 6.0 + rnd(i, 2) * 2.0, col);
                }
                x0 += span;
            }
        }
        _ => {}
    }
}

/// Фон под содержимым панели: брызги / звёзды, луна и замок / светлячки и мухоморы / пыльца, бабочки и цветы.
pub fn decor_bg(ui: &egui::Ui, strong: bool) {
    if !animated() { return; }
    let r = ui.max_rect().expand(24.0).intersect(ui.ctx().screen_rect());
    let p = ui.painter_at(r);
    let t = ui.input(|i| i.time) as f32;
    let k = if strong { 1.0 } else { 0.55 };
    let seed = (r.min.x as u32).wrapping_mul(31) ^ r.width() as u32;
    match current() {
        Theme::Blood => {
            for s in 0..5u32 {
                let cx = r.min.x + rnd(s, 20 + seed) * r.width();
                let cy = r.min.y + (0.2 + rnd(s, 21 + seed) * 0.8) * r.height();
                let base = 8.0 + rnd(s, 22) * 18.0;
                let a = 0.10 * k;
                p.circle_filled(pos2(cx, cy), base, BLOOD.gamma_multiply(a));
                for j in 0..9 {
                    let ang = rnd(s * 31 + j, 23) * TAU;
                    let d = base * (1.2 + rnd(s * 31 + j, 24) * 2.2);
                    p.circle_filled(pos2(cx + ang.cos() * d, cy + ang.sin() * d), base * (0.08 + rnd(s * 31 + j, 25) * 0.2), BLOOD.gamma_multiply(a));
                }
            }
        }
        Theme::Night => {
            for i in 0..40u32 {
                let a = (0.35 + 0.65 * ((t * 0.8 + rnd(i, 30) * 6.0).sin().abs())) * k;
                p.circle_filled(pos2(r.min.x + rnd(i, 32 + seed) * r.width(), r.min.y + rnd(i, 33 + seed) * r.height() * 0.6),
                                0.7 + rnd(i, 31) * 1.1, Color32::WHITE.gamma_multiply(a * 0.8));
            }
            if strong {
                let mc = pos2(r.max.x - r.width() * 0.16, r.min.y + 90.0);
                let mr = (r.width() * 0.07).clamp(26.0, 60.0);
                for g in 1..6 { p.circle_filled(mc, mr * (1.0 + g as f32 * 0.35), MOON.gamma_multiply(0.035)); }
                p.circle_filled(mc, mr, MOON.gamma_multiply(0.9));
                for (dx, dy, rr) in [(-0.3, -0.2, 0.22), (0.35, 0.25, 0.14), (-0.05, 0.45, 0.1)] {
                    p.circle_filled(mc + vec2(dx * mr, dy * mr), mr * rr, Color32::from_rgb(0xcf, 0xc6, 0xa2).gamma_multiply(0.8));
                }
                castle(&p, r, 0.8);
            }
            for b in 0..2 {
                let ph = ((t / 16.0) + b as f32 * 0.5) % 1.0;
                let x = r.min.x - 30.0 + ph * (r.width() + 60.0);
                let y = r.min.y + r.height() * (0.2 + b as f32 * 0.12) + (ph * 6.0 * PI + b as f32).sin() * 14.0;
                bat(&p, pos2(x, y), if b == 1 { 10.0 } else { 14.0 }, (t * 7.0 + b as f32).sin() * 0.5 + 0.5, Color32::from_rgb(7, 5, 12).gamma_multiply(0.6 + 0.4 * k));
            }
        }
        Theme::Forest => {
            if strong {
                // трава и мухоморы по низу
                for i in 0..((r.width() / 7.0) as u32) {
                    let x = r.min.x + i as f32 * 7.0 + rnd(i, 40) * 4.0;
                    let h = 10.0 + rnd(i, 41) * 18.0;
                    let sway = (t * 1.2 + i as f32 * 0.3).sin() * 3.0;
                    p.line_segment([pos2(x, r.max.y), pos2(x + sway, r.max.y - h)], Stroke::new(2.0_f32, Color32::from_rgb(0x1f, 0x4a, 0x2c).gamma_multiply(0.9)));
                }
                for (i, fx) in [0.08f32, 0.22, 0.83].iter().enumerate() {
                    mushroom(&p, pos2(r.min.x + r.width() * fx, r.max.y - 2.0), 16.0 + rnd(i as u32, 42) * 10.0);
                }
            }
            for i in 0..(if strong { 18 } else { 8 }) {
                let fi = i as u32;
                let bx = r.min.x + rnd(fi, 50 + seed) * r.width();
                let by = r.min.y + (0.15 + rnd(fi, 51 + seed) * 0.75) * r.height();
                let x = bx + (t * 0.3 + rnd(fi, 52) * 9.0).sin() * 30.0;
                let y = by + (t * 0.45 + rnd(fi, 53) * 9.0).cos() * 20.0;
                firefly(&p, pos2(x, y), (0.25 + 0.75 * ((t * 1.7 + rnd(fi, 54) * 9.0).sin() * 0.5 + 0.5)) * k);
            }
        }
        Theme::Bloom => {
            if strong {
                for (i, fx) in [0.05f32, 0.12, 0.19, 0.78, 0.86, 0.94].iter().enumerate() {
                    let x = r.min.x + r.width() * fx;
                    let h = 26.0 + rnd(i as u32, 60) * 30.0;
                    p.line_segment([pos2(x, r.max.y), pos2(x + (t + i as f32).sin() * 2.0, r.max.y - h)], Stroke::new(2.0_f32, Color32::from_rgb(0x8f, 0xc9, 0x9b)));
                    leaf(&p, pos2(x, r.max.y - h * 0.4), 9.0, if i % 2 == 0 { 0.7 } else { -0.7 } - PI / 2.0, Color32::from_rgb(0x8f, 0xc9, 0x9b));
                    let col = [Color32::from_rgb(0xf2, 0x9b, 0xc4), Color32::from_rgb(0xc9, 0x9b, 0xf2), Color32::from_rgb(0xff, 0xc8, 0x7a)][i % 3];
                    flower(&p, pos2(x + (t + i as f32).sin() * 2.0, r.max.y - h), 9.0 + rnd(i as u32, 61) * 4.0, col);
                }
                for b in 0..2 {
                    let ph = ((t / 20.0) + b as f32 * 0.5) % 1.0;
                    let x = r.min.x - 20.0 + ph * (r.width() + 40.0);
                    let y = r.min.y + r.height() * (0.3 + b as f32 * 0.25) + (ph * 8.0 * PI).sin() * 24.0;
                    let col = if b == 0 { Color32::from_rgb(0xb0, 0x7f, 0xd8) } else { Color32::from_rgb(0xf2, 0x9b, 0xc4) };
                    butterfly(&p, pos2(x, y), 12.0, (t * 9.0 + b as f32).sin() * 0.5 + 0.5, col.gamma_multiply(0.85));
                }
            }
            // пыльца: блёстки медленно падают и мерцают
            for i in 0..(if strong { 26 } else { 12 }) {
                let fi = i as u32;
                let speed = 12.0 + rnd(fi, 70) * 18.0;
                let y = r.min.y + ((t * speed + rnd(fi, 71 + seed) * r.height()) % r.height());
                let x = r.min.x + rnd(fi, 72 + seed) * r.width() + (t * 0.7 + fi as f32).sin() * 10.0;
                let tw = (t * 3.0 + rnd(fi, 73) * 9.0).sin() * 0.5 + 0.5;
                sparkle(&p, pos2(x, y), 2.0 + tw * 3.0, Color32::from_rgb(0xe8, 0xb0, 0x2e).gamma_multiply((0.35 + 0.6 * tw) * k));
            }
        }
        _ => {}
    }
}

/// Оформление пузыря сообщения (рисуется поверх рамки пузыря `rect`).
pub fn bubble_decor(p: &egui::Painter, rect: Rect, mine: bool, key: &str) {
    let h = key.bytes().fold(7u32, |h, b| h.wrapping_mul(31) ^ b as u32);
    match current() {
        Theme::Blood if mine && h % 4 != 0 => {
            let x = rect.min.x + rect.width() * (0.25 + (h >> 3 & 0xff) as f32 / 255.0 * 0.5);
            drip(p, pos2(x, rect.max.y - 2.0), 6.0, 7.0 + (h >> 11 & 7) as f32);
        }
        Theme::Blood => { p.rect_stroke(rect, CornerRadius::same(10), Stroke::new(1.0_f32, BLOOD.gamma_multiply(0.5)), egui::StrokeKind::Inside); }
        Theme::Night => { p.rect_stroke(rect, CornerRadius::same(10), Stroke::new(1.0_f32, Color32::from_rgb(0xb9, 0xb4, 0xc8).gamma_multiply(if mine { 0.4 } else { 0.25 })), egui::StrokeKind::Inside); }
        Theme::Forest if h % 3 == 0 => leaf(p, pos2(if mine { rect.min.x + 6.0 } else { rect.max.x - 6.0 }, rect.min.y + 4.0), 10.0,
                                             if mine { -2.4 } else { -0.7 }, Color32::from_rgb(0x6f, 0xb8, 0x5a)),
        Theme::Bloom if h % 3 == 0 => sparkle(p, pos2(rect.max.x - 6.0, rect.min.y + 6.0), 5.0, Color32::from_rgb(0xe8, 0xb0, 0x2e)),
        _ => {}
    }
}

// ── кисти ────────────────────────────────────────────────────────────────────

fn drip(p: &egui::Painter, at: Pos2, w: f32, len: f32) {
    let r = w / 2.0;
    let body = vec![pos2(at.x - r * 0.9, at.y - 1.0), pos2(at.x + r * 0.9, at.y - 1.0), pos2(at.x + r * 0.6, at.y + len - r),
                    pos2(at.x - r * 0.6, at.y + len - r)];
    p.add(Shape::convex_polygon(body, BLOOD, Stroke::NONE));
    p.circle_filled(pos2(at.x, at.y + len - r), r, BLOOD);
    p.circle_filled(pos2(at.x - r * 0.35, at.y + len - r * 1.2), r * 0.3, Color32::WHITE.gamma_multiply(0.3));
}

fn bat(p: &egui::Painter, c: Pos2, span: f32, flap: f32, col: Color32) {
    let up = (flap - 0.5) * span * 0.6;
    for s in [-1.0f32, 1.0] {
        let wing = vec![c, pos2(c.x + s * span * 0.4, c.y - span * 0.25 - up), pos2(c.x + s * span, c.y - up),
                        pos2(c.x + s * span * 0.78, c.y + span * 0.12 - up * 0.5), pos2(c.x + s * span * 0.6, c.y + span * 0.02 - up * 0.4),
                        pos2(c.x + s * span * 0.42, c.y + span * 0.18 - up * 0.3), pos2(c.x + s * span * 0.2, c.y + span * 0.08)];
        p.add(Shape::Path(egui::epaint::PathShape { points: wing, closed: true, fill: col, stroke: egui::epaint::PathStroke::NONE }));
    }
    p.circle_filled(pos2(c.x, c.y - span * 0.02), span * 0.15, col);
    for s in [-1.0f32, 1.0] { p.line_segment([pos2(c.x + s * span * 0.07, c.y - span * 0.12), pos2(c.x + s * span * 0.1, c.y - span * 0.27)], Stroke::new(span * 0.06, col)); }
}

/// Силуэт замка по низу прямоугольника: стена с зубцами, башни с крышами и шпилями, окна.
fn castle(p: &egui::Painter, r: Rect, k: f32) {
    let u = (r.width() / 24.0).min(28.0);
    let x0 = r.center().x - 12.0 * u;
    let base = r.max.y;
    let sil = Color32::from_rgb(6, 4, 10).gamma_multiply(0.55 + 0.45 * k);
    p.rect_filled(Rect::from_min_max(pos2(r.min.x, base - 3.2 * u), pos2(r.max.x, base)), CornerRadius::ZERO, sil);
    let mut x = r.min.x;
    while x < r.max.x { p.rect_filled(Rect::from_min_size(pos2(x, base - 3.8 * u), vec2(0.5 * u, 0.6 * u)), CornerRadius::ZERO, sil); x += u; }
    for (i, (cx, wd, h)) in [(3.0f32, 2.2f32, 8.0f32), (8.5, 1.6, 6.0), (13.0, 3.0, 11.0), (17.5, 1.8, 7.0), (21.0, 2.4, 9.0)].iter().enumerate() {
        let l = x0 + (cx - wd / 2.0) * u;
        let rr = x0 + (cx + wd / 2.0) * u;
        let top = base - h * u;
        p.rect_filled(Rect::from_min_max(pos2(l, top), pos2(rr, base)), CornerRadius::ZERO, sil);
        let apex = pos2((l + rr) / 2.0, top - wd * 1.6 * u);
        p.add(Shape::convex_polygon(vec![pos2(l - 0.35 * u, top), apex, pos2(rr + 0.35 * u, top)], sil, Stroke::NONE));
        p.line_segment([apex, apex - vec2(0.0, 0.9 * u)], Stroke::new(2.0_f32, sil));
        for j in 0..((h / 2.5) as u32) {
            if rnd(i as u32 * 7 + j, 40) < 0.35 { continue; }
            let wy = top + (1.2 + j as f32 * 2.4) * u;
            p.rect_filled(Rect::from_min_size(pos2((l + rr) / 2.0 - 0.22 * u, wy), vec2(0.44 * u, 0.8 * u)), CornerRadius::same(4),
                          Color32::from_rgb(0xff, 0xc5, 0x60).gamma_multiply(0.5 * k + 0.1));
        }
    }
}

fn firefly(p: &egui::Painter, c: Pos2, glow: f32) {
    let g = Color32::from_rgb(0xf4, 0xe3, 0x5e);
    p.circle_filled(c, 9.0, g.gamma_multiply(0.06 * glow));
    p.circle_filled(c, 5.0, g.gamma_multiply(0.15 * glow));
    p.circle_filled(c, 2.0, g.gamma_multiply(0.4 + 0.6 * glow));
}

/// Мухомор: ножка, красная шляпка в белых точках.
fn mushroom(p: &egui::Painter, base: Pos2, size: f32) {
    let stem = Color32::from_rgb(0xf1, 0xe6, 0xd2);
    p.add(Shape::convex_polygon(vec![pos2(base.x - size * 0.16, base.y), pos2(base.x + size * 0.16, base.y),
                                     pos2(base.x + size * 0.11, base.y - size * 0.75), pos2(base.x - size * 0.11, base.y - size * 0.75)], stem, Stroke::NONE));
    let cap: Vec<Pos2> = (0..=16).map(|k| {
        let a = PI + k as f32 / 16.0 * PI;
        pos2(base.x + a.cos() * size * 0.6, base.y - size * 0.7 + a.sin() * size * 0.5)
    }).collect();
    p.add(Shape::convex_polygon(cap, Color32::from_rgb(0xc8, 0x2a, 0x22), Stroke::NONE));
    for (dx, dy, r) in [(-0.3f32, -0.95f32, 0.08f32), (0.05, -1.1, 0.07), (0.32, -0.9, 0.06), (-0.05, -0.85, 0.05)] {
        p.circle_filled(pos2(base.x + dx * size, base.y + dy * size), r * size, Color32::WHITE.gamma_multiply(0.9));
    }
}

/// Лист: два дуговых края, `ang` — направление (радианы).
fn leaf(p: &egui::Painter, at: Pos2, len: f32, ang: f32, col: Color32) {
    let d = vec2(ang.cos(), ang.sin());
    let n = vec2(-d.y, d.x);
    let tip = at + d * len;
    let pts: Vec<Pos2> = (0..=8).map(|k| { let u = k as f32 / 8.0; at + d * len * u + n * (u * PI).sin() * len * 0.3 })
        .chain((0..=8).rev().map(|k| { let u = k as f32 / 8.0; at + d * len * u - n * (u * PI).sin() * len * 0.3 })).collect();
    p.add(Shape::convex_polygon(pts, col, Stroke::NONE));
    p.line_segment([at, tip], Stroke::new(0.8_f32, col.gamma_multiply(0.6)));
}

/// Цветок: пять лепестков и серединка.
fn flower(p: &egui::Painter, c: Pos2, r: f32, col: Color32) {
    for k in 0..5 {
        let a = k as f32 / 5.0 * TAU - PI / 2.0;
        p.circle_filled(c + vec2(a.cos(), a.sin()) * r * 0.62, r * 0.5, col);
    }
    p.circle_filled(c, r * 0.38, Color32::from_rgb(0xff, 0xe0, 0x8a));
}

/// Бабочка: две пары крыльев, `flap` 0..1 — взмах (сужение по горизонтали).
fn butterfly(p: &egui::Painter, c: Pos2, s: f32, flap: f32, col: Color32) {
    let wx = 0.35 + 0.65 * flap;
    for side in [-1.0f32, 1.0] {
        let up: Vec<Pos2> = (0..=12).map(|k| { let a = k as f32 / 12.0 * TAU; pos2(c.x + side * (0.55 + a.cos() * 0.5) * s * wx, c.y - s * 0.35 + a.sin() * s * 0.45) }).collect();
        let lo: Vec<Pos2> = (0..=12).map(|k| { let a = k as f32 / 12.0 * TAU; pos2(c.x + side * (0.4 + a.cos() * 0.35) * s * wx, c.y + s * 0.3 + a.sin() * s * 0.3) }).collect();
        p.add(Shape::convex_polygon(up, col, Stroke::NONE));
        p.add(Shape::convex_polygon(lo, col.gamma_multiply(0.85), Stroke::NONE));
    }
    p.line_segment([pos2(c.x, c.y - s * 0.5), pos2(c.x, c.y + s * 0.55)], Stroke::new(s * 0.12, Color32::from_rgb(0x4a, 0x35, 0x50)));
}

/// Блёстка: четырёхлучевая звёздочка.
fn sparkle(p: &egui::Painter, c: Pos2, r: f32, col: Color32) {
    let pts: Vec<Pos2> = (0..8).map(|k| {
        let a = k as f32 / 8.0 * TAU;
        let rr = if k % 2 == 0 { r } else { r * 0.28 };
        pos2(c.x + a.cos() * rr, c.y + a.sin() * rr)
    }).collect();
    p.add(Shape::Path(egui::epaint::PathShape { points: pts, closed: true, fill: col, stroke: egui::epaint::PathStroke::NONE }));
}
