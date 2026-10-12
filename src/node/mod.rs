//! node/mod.rs — TCP-сервер, диспетчер, bucket refresh, graceful shutdown (§6.1, §22 ТЗ).

use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::{Context, Result};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;
use tracing::{debug, error, info, warn};

use crate::config::NodeConfig;
use crate::dht::node::DhtNode;
use crate::identity::node_identity::NodeIdentity;
use crate::protocol;
use crate::rpc::RpcHandler;
use crate::transport::connection::FramedStream;
use crate::transport::framing::{Frame, ReadItem};
use crate::tunnel::manager::TunnelManager;
use crate::types::Contact;

pub struct Node {
    pub dht:         Arc<DhtNode>,
    pub cfg:         Arc<NodeConfig>,
    pub own_contact: Contact,
    pub tunnel_mgr:  Arc<TunnelManager>,
}

impl Node {
    pub fn new(identity: &NodeIdentity, cfg: Arc<NodeConfig>) -> Result<Self> {
        let addr_str = format!("{}:{}", cfg.node.listen_host, cfg.node.listen_port);
        let own_contact = Contact::new(
            identity.node_id,
            identity.public_key_bytes().to_vec(),
            addr_str.parse().context("listen addr parse")?,
        );
        let dht = Arc::new(DhtNode::new(own_contact.clone(), cfg.clone()));
        let tunnel_mgr = Arc::new(TunnelManager::new(
            own_contact.clone(),
            dht.routing.clone(),
            cfg.clone(),
        ));
        Ok(Self { dht, cfg, own_contact, tunnel_mgr })
    }

    pub async fn start(&self) -> Result<()> {
        let listen_addr = format!("{}:{}", self.cfg.node.listen_host, self.cfg.node.listen_port);
        let listener = TcpListener::bind(&listen_addr)
            .await.with_context(|| format!("bind {listen_addr}"))?;
        info!("Listening on {} NodeID={}", listen_addr, self.own_contact.node_id.short());

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
            tokio::spawn(async move {
                if let Err(e) = dht.join(&addrs).await {
                    error!("Bootstrap error: {e}");
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
                            let mut rx2  = shutdown_rx.clone();
                            tokio::spawn(async move {
                                tokio::select! {
                                    res = handle_conn(stream, peer_addr, handler, timeout) => {
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
        )
    }

    fn resolve_bootstrap(&self) -> Vec<SocketAddr> {
        self.cfg.node.bootstrap_peers.iter()
            .filter_map(|p| p.parse().ok())
            .collect()
    }
}

async fn handle_conn(
    stream:  TcpStream,
    peer:    SocketAddr,
    handler: RpcHandler,
    timeout: u64,
) -> Result<()> {
    let mut framed = FramedStream::new(stream, timeout);
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
