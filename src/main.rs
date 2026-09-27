//! main.rs — точка входа узла P2P-сети.
//!
//! Запуск:  cargo run --bin p2p-node -- --config config/node_config.yaml --port 7001
//!
//! Переменные среды для LOG_LEVEL:
//!   RUST_LOG=p2p_overlay=debug cargo run --bin p2p-node

use std::path::PathBuf;
use std::sync::Arc;
use anyhow::Result;
use clap::Parser;
use tracing::info;

use p2p_overlay::config::NodeConfig;
use p2p_overlay::identity::node_identity::NodeIdentity;
use p2p_overlay::node::Node;

#[derive(Parser, Debug)]
#[command(name = "p2p-node", about = "[Coursework] Secure P2P Overlay Node")]
struct Cli {
    /// Путь к YAML-конфигу узла (NODE_STATE_DIR, LISTEN_HOST, LISTEN_PORT, …)
    #[arg(short, long, default_value = "config/node_config.yaml")]
    config: PathBuf,

    /// Переопределить LISTEN_PORT из конфига
    #[arg(short, long)]
    port: Option<u16>,

    /// Bootstrap-пиры (BOOTSTRAP_PEERS), напр.: -b 127.0.0.1:7000
    #[arg(short, long)]
    bootstrap: Vec<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // Загрузить конфиг
    let mut cfg = if cli.config.exists() {
        NodeConfig::from_file(&cli.config)?
    } else {
        NodeConfig::default()
    };
    if let Some(port) = cli.port { cfg.node.listen_port = port; }
    if !cli.bootstrap.is_empty() { cfg.node.bootstrap_peers = cli.bootstrap; }

    // Инициализация логов
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(format!("p2p_overlay={}", cfg.node.log_level).to_lowercase().parse()?)
        )
        .init();

    info!("=== P2P Overlay Node [Coursework] ===");
    info!("Level: {:?}", cfg.level);
    info!("DHT:   K={} α={} R={}", cfg.dht.k_bucket_size, cfg.dht.alpha, cfg.dht.replication_factor);

    // Идентичность узла
    let identity = NodeIdentity::load_or_create(&cfg.node.state_dir)?;
    info!("NodeID = {}", identity.node_id);

    // Инициализировать и запустить узел
    let cfg = Arc::new(cfg);
    let node = Node::new(&identity, cfg.clone())?;
    node.start().await?;

    Ok(())
}
