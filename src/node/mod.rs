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

/// IPv4-адрес в локальной сети для `advertise_host: auto`.
///
/// Маршрут по умолчанию часто идёт через VPN/туннель (tun, wg, xray) — его адрес
/// телефону недоступен. Поэтому сначала перебираем интерфейсы и берём частный адрес
/// (10/8, 172.16/12, 192.168/16) на физическом Wi-Fi/Ethernet, и только затем —
/// адрес интерфейса маршрута по умолчанию.
pub fn lan_ip() -> Option<std::net::IpAddr> {
    let mut best: Option<(u8, std::net::Ipv4Addr)> = None;
    for (name, ip) in interfaces_v4() {
        if ip.is_loopback() || ip.is_link_local() || ip.is_unspecified() { continue; }
        let n = name.to_ascii_lowercase();
        let virt = ["tun", "tap", "wg", "docker", "br-", "veth", "virbr", "vboxnet", "vmnet", "xray", "happ",
                    "zt", "tailscale", "utun", "ppp", "dummy", "lxc", "cni", "flannel", "rmnet"]
            .iter().any(|p| n.starts_with(p) || n.contains(p));
        let phys = ["wl", "en", "eth", "ap", "swlan"].iter().any(|p| n.starts_with(p));
        let rank = match (ip.is_private(), phys, virt) {
            (true, true, false) => 3,
            (true, false, false) => 2,
            (_, _, false) => 1,
            _ => 0,
        };
        if rank > 0 && best.map(|(r, _)| rank > r).unwrap_or(true) { best = Some((rank, ip)); }
    }
    if let Some((_, ip)) = best { return Some(ip.into()); }
    // запасной путь: адрес интерфейса маршрута по умолчанию (UDP connect не отправляет пакетов)
    let s = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    s.connect("192.0.2.1:9").ok()?; // TEST-NET-1: только выбор интерфейса
    let ip = s.local_addr().ok()?.ip();
    (!ip.is_unspecified() && !ip.is_loopback()).then_some(ip)
}

/// (имя интерфейса, IPv4) всех поднятых интерфейсов.
#[cfg(unix)]
fn interfaces_v4() -> Vec<(String, std::net::Ipv4Addr)> {
    let mut out = vec![];
    // SAFETY: getifaddrs/freeifaddrs — стандартный POSIX API; список читается до освобождения.
    unsafe {
        let mut ifap: *mut libc::ifaddrs = std::ptr::null_mut();
        if libc::getifaddrs(&mut ifap) != 0 { return out; }
        let mut p = ifap;
        while !p.is_null() {
            let ifa = &*p;
            let up = ifa.ifa_flags & (libc::IFF_UP as libc::c_uint) != 0;
            if up && !ifa.ifa_addr.is_null() && (*ifa.ifa_addr).sa_family as i32 == libc::AF_INET {
                let sin = &*(ifa.ifa_addr as *const libc::sockaddr_in);
                let ip = std::net::Ipv4Addr::from(u32::from_be(sin.sin_addr.s_addr));
                let name = std::ffi::CStr::from_ptr(ifa.ifa_name).to_string_lossy().into_owned();
                out.push((name, ip));
            }
            p = ifa.ifa_next;
        }
        libc::freeifaddrs(ifap);
    }
    out
}

#[cfg(not(unix))]
fn interfaces_v4() -> Vec<(String, std::net::Ipv4Addr)> { vec![] }

/// Какой адрес сообщать пирам: конкретный listen_host; при 0.0.0.0 — advertise_host
/// (`auto` — LAN-адрес) или 127.0.0.1.
pub fn advertised_host(listen_host: &str, advertise: &str) -> String {
    if listen_host != "0.0.0.0" { return listen_host.to_owned(); }
    match advertise.trim() {
        "" => "127.0.0.1".to_owned(),
        "auto" => lan_ip().map(|ip| { tracing::info!("advertise auto → {ip}"); ip.to_string() }).unwrap_or_else(|| {
            tracing::warn!("advertise_host=auto: LAN-адрес не найден, анонсируем 127.0.0.1");
            "127.0.0.1".to_owned()
        }),
        h => h.to_owned(),
    }
}

impl Node {
    pub fn new(identity: &NodeIdentity, cfg: Arc<NodeConfig>) -> Result<Self> {
        // A3: 0.0.0.0 — адрес для bind, но не для анонса пирам (его нельзя набрать):
        // анонсируем advertise_host (LAN-адрес для других устройств) или 127.0.0.1
        let adv_host = advertised_host(&cfg.node.listen_host, &cfg.node.advertise_host);
        let addr_str = format!("{}:{}", adv_host, cfg.node.listen_port);
        let mut own_contact = Contact::new(
            identity.node_id,
            identity.public_key_bytes().to_vec(),
            addr_str.parse().context("listen addr parse")?,
        );
        own_contact.relay = cfg.tunnel.relay;
        if !cfg.tunnel.relay { info!("Режим «только клиент»: чужие туннели через этот узел не строятся"); }
        // Канал: TLS 1.3 (mutual, сертификат на identity-ключе) или собственное рукопожатие (§11.3)
        match cfg.security.channel.as_str() {
            "tls" => crate::rpc::install_tls(Arc::new(
                crate::security::tls::TlsSecurity::new(identity.signing_key()).context("TLS init")?,
            )),
            "ake" => crate::rpc::install_ake(Arc::new(crate::security::ake::AkeSecurity::from_identity(identity))),
            other => anyhow::bail!("security.channel = {other}: допустимо tls или ake"),
        }
        // соединения между узлами: на каждый запрос или пул (transport.reuse_connections, ТЗ §6 допускает оба)
        crate::rpc::pool::install(&cfg.transport);
        // узел на конкретном адресе (не 0.0.0.0) и соединяется с него — для сценария с ограниченным underlay
        if let Ok(ip) = cfg.node.listen_host.parse::<std::net::IpAddr>() {
            if !ip.is_unspecified() { crate::rpc::set_local_ip(ip); }
        }
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
        ).with_history(crate::app::history::History::open(&cfg.node.state_dir))
         .with_replay_log(&cfg.node.state_dir)
         .with_outbox(&cfg.node.state_dir));
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

        // ── §10.1: TTL-очистка хранилища и переопубликование своих записей; §13.2: неполные файлы ─
        {
            let dht = self.dht.clone();
            let msgr = self.messenger.clone();
            let mut rx = shutdown_rx.clone();
            tokio::spawn(async move {
                let mut t = tokio::time::interval(std::time::Duration::from_secs(10));
                loop {
                    tokio::select! {
                        _ = t.tick() => { dht.store.evict_expired().await; msgr.evict_stale_files().await; }
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

        // ── Офлайн-доставка: проверка почтовых ящиков DHT и срок жизни своей очереди ───
        if self.cfg.mailbox.enabled {
            let msgr = self.messenger.clone();
            let secs = self.cfg.mailbox.poll_interval_s.max(10);
            let mut rx = shutdown_rx.clone();
            tokio::spawn(async move {
                // сначала дать узлу войти в сеть
                tokio::time::sleep(std::time::Duration::from_secs(4)).await;
                msgr.republish_outbox().await;
                let mut t = tokio::time::interval(std::time::Duration::from_secs(secs));
                loop {
                    tokio::select! {
                        _ = t.tick() => { msgr.poll_mailboxes().await; msgr.expire_outbox().await; }
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
                            let _ = stream.set_nodelay(true);   // см. rpc::dial_expect: без Нейгла и отложенных ответов
                            let handler  = self.make_handler();
                            let timeout  = self.cfg.transport.read_timeout_ms;
                            let handshake_ms = self.cfg.transport.connect_timeout_ms;
                            // Режим соединений — один на всю сеть (ТЗ §6). Без пула — как раньше: простой между кадрами
                            // не дольше READ_TIMEOUT_MS и запросы соединения по одному. С пулом — простой до
                            // idle_timeout_ms и до 64 запросов соединения одновременно. Недочитанный кадр — всегда READ_TIMEOUT_MS.
                            let pooled = self.cfg.transport.reuse_connections;
                            let idle_ms = if pooled { self.cfg.transport.idle_timeout_ms } else { self.cfg.transport.read_timeout_ms };
                            let inflight = if pooled { 64 } else { 1 };
                            let mut rx2  = shutdown_rx.clone();
                            tokio::spawn(async move {
                                let hs_timeout = std::time::Duration::from_millis(handshake_ms);
                                tokio::select! {
                                    res = async move {
                                        let framed = if let Some(a) = crate::rpc::ake() {
                                            let (s, id) = tokio::time::timeout(hs_timeout, a.accept(stream)).await
                                                .map_err(|_| anyhow::anyhow!("AKE handshake timeout"))??;
                                            debug!("AKE peer {} NodeID={}", peer_addr, id.short());
                                            FramedStream::new(Box::new(s) as BoxedIo, timeout).with_peer(id)
                                        } else { match crate::rpc::tls() {
                                            Some(t) => {
                                                let (tls, id) = tokio::time::timeout(hs_timeout, t.accept(stream)).await
                                                    .map_err(|_| anyhow::anyhow!("TLS handshake timeout"))??;
                                                debug!("TLS peer {} NodeID={}", peer_addr, id.short());
                                                FramedStream::new(Box::new(tls) as BoxedIo, timeout).with_peer(id)
                                            }
                                            None => FramedStream::new(Box::new(stream) as BoxedIo, timeout),
                                        } };
                                        handle_conn(framed, peer_addr, handler, idle_ms, inflight).await
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
    framed:  FramedStream<BoxedIo>,
    peer:    SocketAddr,
    handler: RpcHandler,
    idle_ms: u64,
    max_inflight: usize,
) -> Result<()> {
    // Чтение и запись — в разных задачах, каждый запрос обрабатывается отдельно: соседи с пулом соединений
    // шлют по одному соединению много запросов, и ретранслятор, ждущий ответа следующего узла, не должен
    // задерживать остальные. Не больше `max_inflight` запросов одного соединения одновременно
    // (без пула — 1: запросы соединения строго по очереди, как до введения пула).
    let max_inflight = max_inflight.max(1);
    let (io, rest, _) = framed.into_parts();
    let (rh, mut wh) = tokio::io::split(io);
    let mut rd = rest.with(rh);
    let (tx, mut rx) = tokio::sync::mpsc::channel::<Frame>(max_inflight.max(8));
    let writer = tokio::spawn(async move {
        while let Some(f) = rx.recv().await {
            if crate::transport::connection::write_frame(&mut wh, &f).await.is_err() { break; }
        }
        use tokio::io::AsyncWriteExt;
        let _ = wh.shutdown().await;
    });
    let handler = Arc::new(handler);
    let slots = Arc::new(tokio::sync::Semaphore::new(max_inflight));
    let res = loop {
        match rd.next(idle_ms).await {
            Some(ReadItem::Frame(frame)) => {
                if frame.version != protocol::PROTOCOL_VERSION {
                    warn!("Bad version {} from {} — closing (§10 TZ)", frame.version, peer);
                    let _ = tx.send(Frame::error_version(frame.request_id)).await;
                    break Err(anyhow::anyhow!("bad protocol version"));
                }
                let Ok(permit) = slots.clone().acquire_owned().await else { break Ok(()) };
                let (h, tx) = (handler.clone(), tx.clone());
                tokio::spawn(async move {
                    if let Some(resp) = h.handle(frame).await { let _ = tx.send(resp).await; }
                    drop(permit);
                });
            }
            Some(ReadItem::Error(e)) => {
                warn!("Frame error from {} (§10 TZ — closing): {e}", peer);
                break Err(anyhow::anyhow!("protocol error: {e}"));
            }
            Some(ReadItem::Eof) => break Ok(()),
            None => { debug!("Connection {} idle — closing", peer); break Ok(()); }
        }
    };
    // ответы уже начатых запросов дописываются, затем соединение закрывается
    drop(tx);
    let _ = writer.await;
    res
}
