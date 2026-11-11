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

    /// Порт локального веб-интерфейса узла (127.0.0.1). Без флага GUI выключен.
    #[arg(long)]
    gui_port: Option<u16>,

    /// Переопределить NODE_STATE_DIR
    #[arg(long)]
    state_dir: Option<PathBuf>,

    /// Переопределить каталог метрик
    #[arg(long)]
    metrics_dir: Option<PathBuf>,

    /// Адрес узла (переопределяет LISTEN_HOST), напр. 127.0.1.5: узел слушает и соединяется с этого адреса —
    /// так на одной машине моделируются разные подсети (сценарий underlay)
    #[arg(long)]
    host: Option<String>,

    /// Адрес для пиров при прослушивании 0.0.0.0: IP в локальной сети или `auto`
    /// (подключение телефона/другого компьютера). Без флага — 127.0.0.1.
    #[arg(long)]
    advertise: Option<String>,

    /// Канал между узлами: tls (TLS 1.3) или ake (собственное рукопожатие, ТЗ §11.3); переопределяет конфиг
    #[arg(long)]
    channel: Option<String>,

    /// Стратегия выбора ретрансляторов: closest | random | rating (переопределяет tunnel.relay_selection)
    #[arg(long)]
    relay_selection: Option<String>,

    /// Размер пула туннелей к одному получателю (переопределяет tunnel.pool_size)
    #[arg(long)]
    pool_size: Option<usize>,

    /// Переиспользовать соединения между узлами (пул; переопределяет transport.reuse_connections)
    #[arg(long)]
    reuse_connections: bool,

    /// Не переиспользовать соединения: новое на каждый запрос (как до 11.11.2026; для сравнения режимов)
    #[arg(long)]
    no_reuse_connections: bool,

    /// Что идёт через пул: tunnels (по умолчанию; DHT — соединение на каждый запрос) | all
    #[arg(long)]
    pool_scope: Option<String>,

    /// Режим «только клиент»: не быть ретранслятором чужих туннелей (экономия батареи и трафика)
    #[arg(long)]
    no_relay: bool,

    /// Период обновления бакетов DHT, с (переопределяет dht.refresh_interval_seconds)
    #[arg(long)]
    refresh_interval: Option<u64>,

    /// Период проверки почтовых ящиков DHT (офлайн-доставка), с; 0 — выключить офлайн-доставку
    #[arg(long)]
    mailbox_poll: Option<u64>,

    /// Эксперимент: вероятность, что этот узел как ретранслятор откажет в пересылке (0..1)
    #[arg(long)]
    fault_drop: Option<f64>,

    /// Эксперимент: задержка каждой пересылки этим узлом как ретранслятором, мс
    #[arg(long)]
    fault_delay_ms: Option<u64>,

    /// Экспортировать таблицу маршрутизации в JSON и выйти (§26 ТЗ)
    #[arg(long)]
    export_routing: Option<std::path::PathBuf>,
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
    if let Some(d) = cli.state_dir { cfg.node.state_dir = d; }
    if let Some(d) = cli.metrics_dir { cfg.node.metrics_dir = d; }
    if !cli.bootstrap.is_empty() { cfg.node.bootstrap_peers = cli.bootstrap; }
    if let Some(c) = cli.channel { cfg.security.channel = c; }
    if let Some(h) = cli.host { cfg.node.listen_host = h; }
    if let Some(a) = cli.advertise { cfg.node.advertise_host = a; }
    if let Some(s) = cli.relay_selection { cfg.tunnel.relay_selection = s; }
    if let Some(p) = cli.pool_size { cfg.tunnel.pool_size = p; }
    if let Some(d) = cli.fault_drop { cfg.fault.drop = d.clamp(0.0, 1.0); }
    if let Some(d) = cli.fault_delay_ms { cfg.fault.delay_ms = d; }
    if cli.no_relay { cfg.tunnel.relay = false; }
    if cli.reuse_connections { cfg.transport.reuse_connections = true; }
    if cli.no_reuse_connections { cfg.transport.reuse_connections = false; }
    if let Some(s) = cli.pool_scope { cfg.transport.pool_scope = s; }
    if !["tunnels", "all"].contains(&cfg.transport.pool_scope.as_str()) {
        anyhow::bail!("transport.pool_scope = {}: допустимо tunnels, all", cfg.transport.pool_scope);
    }
    if let Some(s) = cli.refresh_interval { cfg.dht.refresh_interval_seconds = s.max(10); }
    match cli.mailbox_poll {
        Some(0) => cfg.mailbox.enabled = false,
        Some(s) => cfg.mailbox.poll_interval_s = s.max(10),
        None => {}
    }
    if !["closest", "random", "rating"].contains(&cfg.tunnel.relay_selection.as_str()) {
        anyhow::bail!("tunnel.relay_selection = {}: допустимо closest, random, rating", cfg.tunnel.relay_selection);
    }

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
    info!("Канал: {} · туннели: {} relay, пул {}, выбор {}", cfg.security.channel, cfg.tunnel.min_relays,
          cfg.tunnel.pool_size, cfg.tunnel.relay_selection);
    if cfg.fault.drop > 0.0 || cfg.fault.delay_ms > 0 {
        tracing::warn!("ВНЕСЕНИЕ ОТКАЗОВ (эксперимент): drop={} delay_ms={}", cfg.fault.drop, cfg.fault.delay_ms);
    }

    // Идентичность узла
    let identity = NodeIdentity::load_or_create(&cfg.node.state_dir)?;
    info!("NodeID = {}", identity.node_id);

    // Инициализировать узел
    let cfg = Arc::new(cfg);
    let node = Node::new(&identity, cfg.clone())?;

    // --export-routing: вывести таблицу и выйти (§26 ТЗ)
    if let Some(path) = cli.export_routing {
        let json = node.dht.export_routing_table().await;
        let s = serde_json::to_string_pretty(&json)?;
        std::fs::write(&path, &s)?;
        println!("Routing table exported to {}", path.display());
        return Ok(());
    }

    let node = Arc::new(node);
    if let Some(gp) = cli.gui_port {
        let n = node.clone();
        tokio::spawn(async move {
            if let Err(e) = p2p_overlay::gui::node_gui::run(n, gp).await {
                tracing::error!("GUI: {e}");
            }
        });
    }

    node.start().await?;

    Ok(())
}
