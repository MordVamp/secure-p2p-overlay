//! orchestrator.rs — оркестратор кластера (Фаза 8) с веб-интерфейсом.
//!
//! Запускает N логических узлов (процессы `p2p-node`), управляется из браузера.
//! Оркестратор может знать все NodeID, но эта информация НЕ передаётся
//! рабочим узлам (требование ТЗ §15.1): узлам даётся только адрес seed-узла.
//!
//! Запуск:  cargo run --bin p2p-orchestrator -- --gui-port 8080

use std::path::PathBuf;

use clap::Parser;
use p2p_overlay::gui::orch_gui::{self, Orchestrator};

#[derive(Parser, Debug)]
#[command(name = "p2p-orchestrator", about = "Веб-оркестратор кластера P2P-узлов")]
struct Cli {
    /// Порт веб-интерфейса (только 127.0.0.1)
    #[arg(long, default_value_t = 8080)]
    gui_port: u16,
    /// Базовый конфиг для запускаемых узлов
    #[arg(short, long, default_value = "config/node_config.yaml")]
    config: PathBuf,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().init();
    let cli = Cli::parse();
    let orch = Orchestrator::new(cli.config);
    println!("Оркестратор: http://127.0.0.1:{}/", cli.gui_port);
    tokio::select! {
        r = orch_gui::run(orch, cli.gui_port) => r,
        _ = tokio::signal::ctrl_c() => Ok(()),   // kill_on_drop остановит узлы
    }
}
