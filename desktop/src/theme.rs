//! theme.rs — тёмная тема, шрифты и мелкие виджеты (аватар, карточка, «таблетка»).

use eframe::egui::{self, Color32, CornerRadius, FontFamily, FontId, Margin, RichText, Sense, Stroke, TextStyle, Vec2};

pub const BG_RAIL: Color32 = Color32::from_rgb(0x1a, 0x1b, 0x1f);
pub const BG_SIDE: Color32 = Color32::from_rgb(0x24, 0x26, 0x2b);
pub const BG_MAIN: Color32 = Color32::from_rgb(0x2c, 0x2e, 0x34);
pub const BG_CARD: Color32 = Color32::from_rgb(0x36, 0x38, 0x3f);
pub const BG_INPUT: Color32 = Color32::from_rgb(0x1f, 0x20, 0x24);
pub const BG_HOVER: Color32 = Color32::from_rgb(0x3d, 0x40, 0x48);
/// Фон кнопок/переключателей: светлее карточки, чтобы не сливаться с ней.
pub const BG_BUTTON: Color32 = Color32::from_rgb(0x47, 0x4a, 0x53);
pub const ACCENT: Color32 = Color32::from_rgb(0x6c, 0x7b, 0xff);
pub const ACCENT_DIM: Color32 = Color32::from_rgb(0x4a, 0x55, 0xc9);
pub const GREEN: Color32 = Color32::from_rgb(0x3b, 0xc4, 0x7a);
pub const RED: Color32 = Color32::from_rgb(0xf0, 0x56, 0x5a);
pub const YELLOW: Color32 = Color32::from_rgb(0xf2, 0xb8, 0x3c);
pub const TEXT: Color32 = Color32::from_rgb(0xe3, 0xe5, 0xe8);
pub const MUTED: Color32 = Color32::from_rgb(0x9a, 0xa0, 0xaa);

const AVATAR_COLORS: [Color32; 8] = [
    Color32::from_rgb(0xef, 0x6c, 0x6c), Color32::from_rgb(0xf0, 0x9a, 0x4a), Color32::from_rgb(0xd9, 0xb3, 0x3c),
    Color32::from_rgb(0x52, 0xb7, 0x88), Color32::from_rgb(0x3f, 0xa7, 0xc9), Color32::from_rgb(0x6c, 0x7b, 0xff),
    Color32::from_rgb(0xa8, 0x6b, 0xe0), Color32::from_rgb(0xe0, 0x6b, 0xb4),
];

/// Подключить системный шрифт (Adwaita Sans / Inter / Noto / DejaVu), если есть; иначе — встроенный egui.
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
    ctx.set_fonts(fonts);
}

pub fn apply(ctx: &egui::Context) {
    install_fonts(ctx);
    let mut v = egui::Visuals::dark();
    v.panel_fill = BG_MAIN;
    v.window_fill = BG_SIDE;
    v.extreme_bg_color = BG_INPUT;
    v.faint_bg_color = Color32::from_rgb(0x31, 0x33, 0x39);
    v.override_text_color = Some(TEXT);
    v.hyperlink_color = ACCENT;
    v.selection.bg_fill = ACCENT_DIM;
    v.selection.stroke = Stroke::new(1.0_f32, TEXT);
    v.window_stroke = Stroke::new(1.0_f32, BG_HOVER);
    let r = CornerRadius::same(8);
    for (w, fill) in [
        (&mut v.widgets.noninteractive, BG_CARD),
        (&mut v.widgets.inactive, BG_BUTTON),
        (&mut v.widgets.hovered, Color32::from_rgb(0x52, 0x56, 0x60)),
        (&mut v.widgets.active, ACCENT_DIM),
        (&mut v.widgets.open, BG_HOVER),
    ] {
        w.corner_radius = r;
        w.weak_bg_fill = fill;
        w.bg_fill = fill;
    }
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, Color32::from_rgb(0x3a, 0x3c, 0x43));
    v.widgets.inactive.bg_stroke = Stroke::NONE;
    v.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, ACCENT);
    v.widgets.inactive.fg_stroke = Stroke::new(1.0_f32, TEXT);
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, TEXT);
    ctx.set_visuals(v);
    ctx.style_mut(|s| {
        s.spacing.item_spacing = Vec2::new(8.0, 8.0);
        s.spacing.button_padding = Vec2::new(14.0, 7.0);
        s.spacing.interact_size.y = 32.0;
        s.text_styles.insert(TextStyle::Body, FontId::proportional(15.0));
        s.text_styles.insert(TextStyle::Button, FontId::proportional(15.0));
        s.text_styles.insert(TextStyle::Heading, FontId::proportional(22.0));
        s.text_styles.insert(TextStyle::Small, FontId::proportional(12.0));
        s.text_styles.insert(TextStyle::Monospace, FontId::monospace(13.0));
    });
}

pub fn color_for(id: &str) -> Color32 {
    let h = id.bytes().fold(5381u32, |h, b| h.wrapping_mul(33) ^ b as u32);
    AVATAR_COLORS[(h as usize) % AVATAR_COLORS.len()]
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
        .fill(BG_CARD)
        .corner_radius(CornerRadius::same(12))
        .inner_margin(Margin::same(16))
        .show(ui, |ui| { ui.set_width(ui.available_width()); add(ui) })
}

/// Цветная «таблетка» со статусом.
pub fn pill(ui: &mut egui::Ui, text: &str, color: Color32) {
    egui::Frame::default()
        .fill(color.gamma_multiply(0.18))
        .stroke(Stroke::new(1.0_f32, color.gamma_multiply(0.6)))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::symmetric(8, 2))
        .show(ui, |ui| { ui.label(RichText::new(text).color(color).size(12.5)); });
}

/// Однострочное поле ввода с внутренними отступами.
pub fn field(s: &mut String) -> egui::TextEdit<'_> {
    egui::TextEdit::singleline(s).margin(Margin::symmetric(10, 7))
}

pub fn section_title(ui: &mut egui::Ui, text: &str) {
    ui.label(RichText::new(text.to_uppercase()).size(12.0).color(MUTED).strong());
}

pub fn primary(text: &str) -> egui::Button<'static> {
    egui::Button::new(RichText::new(text.to_owned()).color(Color32::WHITE)).fill(ACCENT_DIM)
}

pub fn danger(text: &str) -> egui::Button<'static> {
    egui::Button::new(RichText::new(text.to_owned()).color(Color32::WHITE)).fill(RED.gamma_multiply(0.75))
}

pub fn state_color(state: &str) -> Color32 {
    match state {
        "Active" => GREEN,
        "Degraded" | "Building" => YELLOW,
        _ => RED,
    }
}
