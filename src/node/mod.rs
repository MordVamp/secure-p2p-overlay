//! node/mod.rs — TCP-сервер + диспетчер (§6.1, §22 ТЗ).

use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::{Context, Result};
use tokio::net::{TcpListener, TcpStream};
use tracing::{debug, error, info, warn};

use crate::config::NodeConfig;
use crate::dht::node::DhtNode;
use crate::identity::node_identity::NodeIdentity;
use crate::protocol;
use crate::rpc::{RpcHandler};
use crate::transport::connection::FramedStream;
use crate::transport::framing::{Frame, ReadItem};
use crate::types::Contact;

pub struct Node {
    pub dht:         Arc<DhtNode>,
    pub cfg:         Arc<NodeConfig>,
    pub own_contact: Contact,
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
        Ok(Self { dht, cfg, own_contact })
    }

    pub async fn start(&self) -> Result<()> {
        let listen_addr = format!("{}:{}", self.cfg.node.listen_host, self.cfg.node.listen_port);
        let listener = TcpListener::bind(&listen_addr)
            .await.with_context(|| format!("bind {listen_addr}"))?;
        info!("Listening on {} NodeID={}", listen_addr, self.own_contact.node_id.short());

        // Bootstrap в фоне
        let dht = self.dht.clone();
        let bootstrap_addrs = self.resolve_bootstrap();
        tokio::spawn(async move {
            if let Err(e) = dht.join(&bootstrap_addrs).await {
                error!("Bootstrap error: {e}");
            }
        });

        loop {
            match listener.accept().await {
                Ok((stream, peer_addr)) => {
                    debug!("Connection from {}", peer_addr);
                    let handler = self.make_handler();
                    let timeout = self.cfg.transport.read_timeout_ms;
                    tokio::spawn(async move {
                        if let Err(e) = handle_conn(stream, peer_addr, handler, timeout).await {
                            debug!("Connection {} done: {}", peer_addr, e);
                        }
                    });
                }
                Err(e) => error!("Accept error: {e}"),
            }
        }
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
    stream:   TcpStream,
    peer:     SocketAddr,
    handler:  RpcHandler,
    timeout:  u64,
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
            ReadItem::Eof     => return Ok(()),
        }
    }
}
