//! p2p-desktop — настольный клиент P2P-оверлея (egui).
//!
//! Работает только через локальный HTTP API узла (`p2p-node --gui-port`), поэтому
//! не зависит от внутренностей узла и может как запустить свой узел, так и
//! подключиться к уже работающему. Запуск: `cargo run -p p2p-desktop [-- --profile NAME]`.

mod api;
mod app;
mod node_proc;
mod settings;
mod theme;

fn main() -> eframe::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if let Some(i) = args.iter().position(|a| a == "--profile") {
        match args.get(i + 1).filter(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')) {
            Some(p) => settings::set_profile(p),
            None => { eprintln!("--profile: нужно имя из [A-Za-z0-9_-]"); std::process::exit(2); }
        }
    }
    let title = match settings::profile() {
        "default" => "P2P Overlay".to_owned(),
        p => format!("P2P Overlay — {p}"),
    };
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title(title)
            .with_app_id("p2p-overlay-desktop")
            .with_inner_size([1200.0, 780.0])
            .with_min_inner_size([940.0, 600.0]),
        ..Default::default()
    };
    eframe::run_native("P2P Overlay", options, Box::new(|cc| Ok(Box::new(app::App::new(cc)))))
}
