//! orchestrator.rs — оркестратор кластера (Фаза 8) с веб-интерфейсом.
//!
//! Запускает N логических узлов (процессы `p2p-node`). Оркестратор может знать
//! все NodeID, но эта информация НЕ передаётся рабочим узлам (ТЗ §15.1):
//! узлам даётся только адрес seed-узла.
//!
//! Веб-интерфейс:   cargo run --bin p2p-orchestrator -- --gui-port 8080
//! Приёмка (этапы 1–2, ТЗ §25), реальные измерения:
//!                  cargo run --bin p2p-orchestrator -- acceptance --nodes 13 --lookups 30
//! Сценарии эксперимента 01–09 (ТЗ §15.1) и pcap:
//!                  cargo run --bin p2p-orchestrator -- scenario all --nodes 15
//! Графики и сводка из сохранённых CSV:
//!                  cargo run --bin p2p-orchestrator -- plot experiments/results/06_tunnel_message/<время>

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
    /// Сценарий эксперимента (ТЗ §15.1) на реальных процессах: 01…09, pcap или all (= 01–09).
    /// Результаты — experiments/results/<сценарий>/<время>/ (results.csv, summary.md, meta.json, plots/, node_logs/).
    Scenario {
        which: String,
        /// Число узлов N (устные требования: 13–15, ближе к верхней границе)
        #[arg(long, default_value_t = 15)]
        nodes: usize,
        /// Наблюдений: lookup (02, 05 — на уровень), операций (03), пар сообщений (06)
        #[arg(long, default_value_t = 30)]
        n: usize,
        /// Независимых запусков холодного старта (01, 09 — на схему)
        #[arg(long, default_value_t = 5)]
        runs: usize,
        /// Инъекций отказа (04, 07)
        #[arg(long, default_value_t = 10)]
        inj: usize,
        #[arg(long, default_value_t = 1)]
        seed: u64,
        /// Канал между узлами: tls (по умолчанию из конфига) или ake — собственное рукопожатие
        #[arg(long)]
        channel: Option<String>,
        /// Конфиг узлов, напр. config/node_config_advanced.yaml (продвинутый уровень)
        #[arg(long)]
        node_config: Option<String>,
    },
    /// Перестроить графики (SVG) из сохранённых CSV каталога(ов) результатов; для accept_* — ещё stats.md.
    Plot { dirs: Vec<PathBuf> },
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
        Some(Cmd::Scenario { which, nodes, n, runs, inj, seed, channel, node_config }) => {
            let args = p2p_overlay::experiment::Args { nodes, n, runs, inj, seed, channel, config: node_config };
            let ok = p2p_overlay::experiment::scenarios::run(&which, &args).await?;
            if !ok { eprintln!("сценарий {which}: проверка не пройдена (см. summary.md)"); std::process::exit(1); }
            Ok(())
        }
        Some(Cmd::Plot { dirs }) => {
            for d in dirs {
                let charts = p2p_overlay::experiment::plot::plot_dir(&d)?;
                println!("{}: {}", d.display(), if charts.is_empty() { "графиков для этого каталога нет".into() }
                         else { charts.iter().map(|c| format!("plots/{}", c.0)).collect::<Vec<_>>().join(", ") });
            }
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
