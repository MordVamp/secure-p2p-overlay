//! orchestrator.rs — оркестратор кластера (Фаза 8) с веб-интерфейсом.
//!
//! Запускает N логических узлов (процессы `p2p-node`). Оркестратор может знать
//! все NodeID, но эта информация НЕ передаётся рабочим узлам (ТЗ §15.1):
//! узлам даётся только адрес seed-узла.
//!
//! Веб-интерфейс:   cargo run --bin p2p-orchestrator -- --gui-port 8080
//! Приёмка (этапы 1–2, ТЗ §25), реальные измерения:
//!                  cargo run --bin p2p-orchestrator -- acceptance --nodes 13 --lookups 30

use std::path::PathBuf;

use clap::{Parser, Subcommand};
use p2p_overlay::acceptance::{self, Params};
use p2p_overlay::gui::orch_gui::{self, Orchestrator};

#[derive(Parser, Debug)]
#[command(name = "p2p-orchestrator", about = "Оркестратор кластера P2P-узлов")]
struct Cli {
    /// Порт веб-интерфейса (только 127.0.0.1)
    #[arg(long, default_value_t = 8080)]
    gui_port: u16,
    /// Базовый конфиг для запускаемых узлов
    #[arg(short, long, default_value = "config/node_config.yaml")]
    config: PathBuf,
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// Приёмочный прогон: N узлов, сходимость, выгрузка таблиц, контрольные lookup,
    /// отключение bootstrap-узла и повтор. Результаты — в каталог `--out`.
    Acceptance {
        #[arg(long, default_value_t = 13)]
        nodes: usize,
        #[arg(long, default_value_t = 30)]
        lookups: usize,
        /// Каталог результатов (по умолчанию experiments/results/accept_<время>)
        #[arg(long)]
        out: Option<PathBuf>,
        /// Максимум ожидания сходимости, с
        #[arg(long, default_value_t = 90)]
        converge_timeout: u64,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().init();
    let cli = Cli::parse();
    match cli.cmd {
        Some(Cmd::Acceptance { nodes, lookups, out, converge_timeout }) => {
            let out = out.unwrap_or_else(|| {
                let ts = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
                PathBuf::from(format!("experiments/results/accept_{ts}"))
            });
            let ok = acceptance::run(Params {
                nodes, lookups, out, config: cli.config, converge_timeout_s: converge_timeout,
            }).await?;
            if !ok { std::process::exit(1); }
            Ok(())
        }
        None => {
            let orch = Orchestrator::new(cli.config);
            println!("Оркестратор: http://127.0.0.1:{}/", cli.gui_port);
            tokio::select! {
                r = orch_gui::run(orch, cli.gui_port) => r,
                _ = tokio::signal::ctrl_c() => Ok(()),   // kill_on_drop остановит узлы
            }
        }
    }
}
