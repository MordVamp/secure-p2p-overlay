//! node/mod.rs — TCP-сервер, диспетчер, bucket refresh, graceful shutdown (§6.1, §22 ТЗ).

use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::{Context, Result};
use tokio::net::TcpListener;
use tokio::sync::watch;
use tracing::{debug, error, info, warn};

use crate::config::NodeConfig;
use crate::dht::node::DhtNode;
use crate::identity::node_identity::NodeIdentity;
use crate::protocol;
use crate::rpc::RpcHandler;
use crate::transport::connection::{BoxedIo, FramedStream};
use crate::transport::framing::{Frame, ReadItem};
use crate::tunnel::manager::TunnelManager;
use crate::types::Contact;

pub struct Node {
    pub dht:         Arc<DhtNode>,
    pub cfg:         Arc<NodeConfig>,
    pub own_contact: Contact,
    pub tunnel_mgr:  Arc<TunnelManager>,
    /// A4: прикладной слой (подписанные сообщения через туннели).
    pub messenger:   Arc<crate::app::messenger::Messenger>,
    pub identity:    Arc<NodeIdentity>,
    routes:          Arc<crate::tunnel::routes::TunnelRoutes>,
    app_tx:          tokio::sync::mpsc::Sender<Vec<u8>>,
    app_rx:          std::sync::Mutex<Option<tokio::sync::mpsc::Receiver<Vec<u8>>>>,
}

impl Node {
    pub fn new(identity: &NodeIdentity, cfg: Arc<NodeConfig>) -> Result<Self> {
        // A3: 0.0.0.0 — адрес для bind, но не для анонса пирам (его нельзя набрать)
        let adv_host = if cfg.node.listen_host == "0.0.0.0" { "127.0.0.1" } else { cfg.node.listen_host.as_str() };
        let addr_str = format!("{}:{}", adv_host, cfg.node.listen_port);
        let own_contact = Contact::new(
            identity.node_id,
            identity.public_key_bytes().to_vec(),
            addr_str.parse().context("listen addr parse")?,
        );
        // TLS 1.3 (mutual, Ed25519): сертификат строится из identity-ключа узла
        crate::rpc::install_tls(Arc::new(
            crate::security::tls::TlsSecurity::new(identity.signing_key())
                .context("TLS init")?,
        ));
        let dht = Arc::new(DhtNode::new(own_contact.clone(), cfg.clone()));
        let tunnel_mgr = Arc::new(TunnelManager::new(
            own_contact.clone(),
            dht.routing.clone(),
            cfg.clone(),
        ));
        let identity = Arc::new(identity.clone());
        let routes   = Arc::new(crate::tunnel::routes::TunnelRoutes::new());
        let (app_tx, app_rx) = tokio::sync::mpsc::channel(256);
        let messenger = Arc::new(crate::app::messenger::Messenger::new(
            identity.clone(), dht.clone(), tunnel_mgr.clone(),
            tunnel_mgr.forwarder.clone(), dht.metrics.clone(), cfg.clone(),
        ).with_history(crate::app::history::History::open(&cfg.node.state_dir)));
        Ok(Self {
            dht, cfg, own_contact, tunnel_mgr, messenger, identity, routes,
            app_tx, app_rx: std::sync::Mutex::new(Some(app_rx)),
        })
    }

    pub async fn start(&self) -> Result<()> {
        let listen_addr = format!("{}:{}", self.cfg.node.listen_host, self.cfg.node.listen_port);
        let listener = TcpListener::bind(&listen_addr)
            .await.with_context(|| format!("bind {listen_addr}"))?;
        info!("Listening on {} NodeID={}", listen_addr, self.own_contact.node_id.short());

        // ── A4: доставка входящих TUNNEL_DATA в Messenger ────────────────
        let app_rx = self.app_rx.lock().ok().and_then(|mut g| g.take());
        if let Some(mut rx) = app_rx {
            let m = self.messenger.clone();
            tokio::spawn(async move {
                while let Some(raw) = rx.recv().await {
                    if let Err(e) = m.handle_incoming(raw).await {
                        warn!("incoming message rejected: {e}");
                    }
                }
            });
        }

        // ── T4: Graceful shutdown через watch-канал ───────────────────────
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let shutdown_tx = Arc::new(shutdown_tx);

        // Ctrl+C handler
        {
            let tx = shutdown_tx.clone();
            tokio::spawn(async move {
                if let Ok(()) = tokio::signal::ctrl_c().await {
                    info!("Ctrl+C — initiating graceful shutdown...");
                    let _ = tx.send(true);
                }
            });
        }

        // ── Bootstrap в фоне ─────────────────────────────────────────────
        {
            let dht  = self.dht.clone();
            let addrs = self.resolve_bootstrap();
            let id   = self.identity.clone();
            tokio::spawn(async move {
                if let Err(e) = dht.join(&addrs).await {
                    error!("Bootstrap error: {e}");
                }
                // §10.3: публикуем свою адресную запись (NodeRecord)
                match dht.publish_node_record(&id).await {
                    Ok(n) => info!("NodeRecord опубликован на {n} репликах"),
                    Err(e) => warn!("NodeRecord: {e}"),
                }
            });
        }

        // ── C3: TunnelManager monitor loop ───────────────────────────────
        {
            let mgr = self.tunnel_mgr.clone();
            let mut rx = shutdown_rx.clone();
            tokio::spawn(async move {
                tokio::select! {
                    _ = mgr.monitor_loop() => {},
                    _ = rx.changed() => {
                        info!("TunnelManager: shutdown signal received");
                    }
                }
            });
        }

        // ── A1: Bucket refresh — периодический lookup случайного ID ──────
        {
            let dht       = self.dht.clone();
            let interval  = self.cfg.dht.refresh_interval_seconds;
            let mut rx    = shutdown_rx.clone();
            tokio::spawn(async move {
                let mut ticker = tokio::time::interval(
                    std::time::Duration::from_secs(interval)
                );
                ticker.tick().await; // пропустить первый тик (сразу после старта)
                loop {
                    tokio::select! {
                        _ = ticker.tick() => {
                            // Lookup случайного ID в каждом непустом бакете
                            let targets = {
                                let rt = dht.routing.lock().await;
                                rt.random_targets_per_bucket(&dht.own_contact.node_id)
                            };
                            info!("Bucket refresh: {} targets", targets.len());
                            for target in targets {
                                let dht2 = dht.clone();
                                tokio::spawn(async move {
                                    if let Err(e) = dht2.find_node(&target).await {
                                        debug!("Bucket refresh lookup error: {e}");
                                    }
                                });
                            }
                        }
                        _ = rx.changed() => {
                            info!("Bucket refresh: shutdown");
                            break;
                        }
                    }
                }
            });
        }

        // ── §10.1: TTL-очистка хранилища и переопубликование своих записей ─
        {
            let dht = self.dht.clone();
            let mut rx = shutdown_rx.clone();
            tokio::spawn(async move {
                let mut t = tokio::time::interval(std::time::Duration::from_secs(10));
                loop {
                    tokio::select! {
                        _ = t.tick() => { dht.store.evict_expired().await; }
                        _ = rx.changed() => break,
                    }
                }
            });
        }
        {
            let dht = self.dht.clone();
            let id  = self.identity.clone();
            let secs = self.cfg.dht.republish_interval_seconds.max(1);
            let mut rx = shutdown_rx.clone();
            tokio::spawn(async move {
                let mut t = tokio::time::interval(std::time::Duration::from_secs(secs));
                t.tick().await;
                loop {
                    tokio::select! {
                        _ = t.tick() => {
                            let n = dht.republish_all(&id).await;
                            if n > 0 { info!("Republished {n} records"); }
                            if let Err(e) = dht.publish_node_record(&id).await { warn!("NodeRecord refresh: {e}"); }
                        }
                        _ = rx.changed() => break,
                    }
                }
            });
        }

        // ── Accept loop ───────────────────────────────────────────────────
        let mut rx_accept = shutdown_rx.clone();
        loop {
            tokio::select! {
                res = listener.accept() => {
                    match res {
                        Ok((stream, peer_addr)) => {
                            debug!("Connection from {}", peer_addr);
                            let handler  = self.make_handler();
                            let timeout  = self.cfg.transport.read_timeout_ms;
                            let handshake_ms = self.cfg.transport.connect_timeout_ms;
                            let mut rx2  = shutdown_rx.clone();
                            tokio::spawn(async move {
                                let hs_timeout = std::time::Duration::from_millis(handshake_ms);
                                tokio::select! {
                                    res = async move {
                                        let framed = match crate::rpc::tls() {
                                            Some(t) => {
                                                let (tls, id) = tokio::time::timeout(hs_timeout, t.accept(stream)).await
                                                    .map_err(|_| anyhow::anyhow!("TLS handshake timeout"))??;
                                                debug!("TLS peer {} NodeID={}", peer_addr, id.short());
                                                FramedStream::new(Box::new(tls) as BoxedIo, timeout).with_peer(id)
                                            }
                                            None => FramedStream::new(Box::new(stream) as BoxedIo, timeout),
                                        };
                                        handle_conn(framed, peer_addr, handler).await
                                    } => {
                                        if let Err(e) = res {
                                            debug!("Connection {} closed: {}", peer_addr, e);
                                        }
                                    }
                                    _ = rx2.changed() => {
                                        debug!("Connection {} shutdown", peer_addr);
                                    }
                                }
                            });
                        }
                        Err(e) => error!("Accept error: {e}"),
                    }
                }
                _ = rx_accept.changed() => {
                    info!("Accept loop: shutdown signal, stopping");
                    break;
                }
            }
        }

        // ── T4: Финальная запись метрик при shutdown ──────────────────────
        info!("Flushing metrics before exit...");
        let snapshot = self.dht.export_routing_table().await;
        self.dht.metrics.snapshot_to_json(snapshot);
        info!("Shutdown complete.");
        Ok(())
    }

    fn make_handler(&self) -> RpcHandler {
        RpcHandler::new(
            self.own_contact.clone(),
            self.dht.routing.clone(),
            self.dht.pending.clone(),
            self.cfg.clone(),
        ).with_services(self.dht.store.clone(), self.routes.clone(), Some(self.app_tx.clone()))
    }

    fn resolve_bootstrap(&self) -> Vec<SocketAddr> {
        self.cfg.node.bootstrap_peers.iter()
            .filter_map(|p| p.parse().ok())
            .collect()
    }
}

async fn handle_conn(
    mut framed: FramedStream<BoxedIo>,
    peer:    SocketAddr,
    handler: RpcHandler,
) -> Result<()> {
    loop {
        match framed.recv_item().await {
            ReadItem::Frame(frame) => {
                if frame.version != protocol::PROTOCOL_VERSION {
                    warn!("Bad version {} from {} — closing (§10 TZ)", frame.version, peer);
                    let _ = framed.send(&Frame::error_version(frame.request_id)).await;
                    return Err(anyhow::anyhow!("bad protocol version"));
                }
                if let Some(resp) = handler.handle(frame).await {
                    framed.send(&resp).await.map_err(|e| anyhow::anyhow!("{e}"))?;
                }
            }
            ReadItem::Error(e) => {
                warn!("Frame error from {} (§10 TZ — closing): {e}", peer);
                return Err(anyhow::anyhow!("protocol error: {e}"));
            }
            ReadItem::Eof => return Ok(()),
        }
    }
}
