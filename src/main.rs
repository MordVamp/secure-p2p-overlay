//! main.rs — точка входа узла оверлейной сети.
//!
//! Запуск: overlay_node --config config/node_config.yaml --port 7001
//! Полный CLI будет реализован в Фазе 7 (прикладной сервис).

use std::path::PathBuf;
use anyhow::Result;
use clap::Parser;
use tracing::info;

use overlay_node::config::NodeConfig;

#[derive(Parser, Debug)]
#[command(name = "overlay_node", about = "P2P overlay network node")]
struct Cli {
    /// Путь к YAML-конфигу узла
    #[arg(short, long, default_value = "config/node_config.yaml")]
    config: PathBuf,

    /// Переопределить порт из конфига
    #[arg(short, long)]
    port: Option<u16>,

    /// Bootstrap-адреса (можно указать несколько: -b 1.2.3.4:7000 -b ...)
    #[arg(short, long)]
    bootstrap: Vec<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    // Логирование
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("overlay_node=debug".parse()?)
        )
        .init();

    let cli = Cli::parse();

    // Загрузить конфиг
    let mut cfg = if cli.config.exists() {
        NodeConfig::from_file(&cli.config)?
    } else {
        info!("Config file not found, using defaults");
        NodeConfig::default()
    };

    if let Some(port) = cli.port {
        cfg.listen_port = port;
    }

    info!("Starting overlay_node on port {}", cfg.listen_port);
    info!("Level: {:?}", cfg.level);
    info!("DHT: k={}, α={}, R={}", cfg.dht.k_bucket_size, cfg.dht.alpha, cfg.dht.replication_factor);

    // ── Инициализация идентичности ────────────────────────────────────────────
    use overlay_node::identity::node_identity::NodeIdentity;
    let identity = NodeIdentity::load_or_create(&cfg.state_dir)?;
    info!("NodeID = {}", identity.node_id);

    // TODO (Фаза 3): инициализировать TlsSecurity
    // TODO (Фаза 4): запустить DHT node
    // TODO (Фаза 6): запустить TunnelManager
    // TODO (Фаза 7): запустить консольный CLI

    info!("Node initialized. Full networking stack coming in Phase 3-7.");
    Ok(())
}
