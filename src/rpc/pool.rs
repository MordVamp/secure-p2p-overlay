//! rpc/pool.rs — запрос к другому узлу: новое соединение на каждый запрос или пул соединений.
//!
//! ТЗ §6 допускает оба подхода. Режим задаёт `transport.reuse_connections`:
//!  - `false`: TCP + рукопожатие канала (TLS/AKE) → один запрос → ответ → закрытие (как до 11.11.2026);
//!  - `true`: одно соединение к соседу на много запросов, в том числе одновременных. Ответы раскладываются
//!    по `request_id` (он уже есть в каждом кадре). Запись пула привязана к паре «адрес + ожидаемый NodeID»:
//!    соединение, установленное с одним NodeID, не используется для запроса к другому (ADR-32 сохраняется).
//!    Простаивающее соединение закрывается через `idle_timeout_ms / 2`, размер пула ограничен `pool_max`.
//!
//! Что идёт через пул, задаёт `transport.pool_scope` (ADR-40): по умолчанию `tunnels` — только трафик туннелей к
//! соседям по цепочке (частые запросы к одним и тем же узлам), а DHT — редкие запросы к разным узлам — остаётся на
//! соединении на каждый запрос: со случайными узлами из lookup соединения не держатся. `all` — всё через пул.
//!
//! Повтор: если переиспользуемое соединение закрылось до ответа (сосед перезапустился, закрыл по простою),
//! запрос один раз повторяется по новому соединению. Это безопасно: TUNNEL_DATA и сообщения защищены от
//! повтора на прикладном уровне (`message_id`), STORE идемпотентен, повторный TUNNEL_BUILD с тем же
//! `tunnel_id` отклоняется и строитель туннеля пробует снова.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use anyhow::{bail, Result};
use tokio::sync::{mpsc, oneshot, Mutex};
use tracing::{debug, info};

use crate::config::TransportConfig;
use crate::transport::connection::write_frame;
use crate::transport::framing::{Frame, ReadItem};
use crate::types::{now_ms, NodeId};

type Key = (SocketAddr, Option<NodeId>);

/// Класс запроса — от него зависит, может ли он идти через пул (`transport.pool_scope`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// PING, FIND_NODE, STORE, FIND_VALUE — редкие запросы к разным узлам.
    Dht,
    /// TUNNEL_BUILD, TUNNEL_DATA — частые запросы к соседям по цепочке.
    Tunnel,
}

struct Conn {
    id:       u64,
    key:      Key,
    peer:     Option<NodeId>,
    /// `None` — сигнал задаче записи закрыть соединение.
    tx:       mpsc::Sender<Option<Frame>>,
    waiters:  std::sync::Mutex<HashMap<[u8; 16], oneshot::Sender<Frame>>>,
    closed:   AtomicBool,
    last_ms:  AtomicU64,
}

impl Conn {
    fn close(&self, why: &str) {
        if self.closed.swap(true, Ordering::SeqCst) { return; }
        // ожидающие получат ошибку «соединение закрыто до ответа»
        self.waiters.lock().map(|mut w| w.clear()).ok();
        let _ = self.tx.try_send(None);
        debug!("conn#{} → {} закрыто: {why}", self.id, self.key.0);
    }
}

struct Pool {
    conns:   Mutex<HashMap<Key, Arc<Conn>>>,
    idle_ms: u64,
    max:     usize,
    /// DHT тоже через пул (`pool_scope: all`).
    dht:     bool,
    next_id: AtomicU64,
    opened:  AtomicU64,
    reused:  AtomicU64,
}

static POOL: OnceLock<Pool> = OnceLock::new();

/// Включить пул (вызывается при создании узла, если `transport.reuse_connections`).
pub fn install(cfg: &TransportConfig) {
    if !cfg.reuse_connections { return; }
    let _ = POOL.set(Pool {
        conns: Mutex::new(HashMap::new()), idle_ms: (cfg.idle_timeout_ms / 2).max(1000), max: cfg.pool_max.max(4),
        dht: cfg.pool_scope == "all",
        next_id: AtomicU64::new(1), opened: AtomicU64::new(0), reused: AtomicU64::new(0),
    });
    info!("Соединения между узлами переиспользуются: {} (пул до {}, простой {} мс)",
          if cfg.pool_scope == "all" { "весь трафик" } else { "туннели; DHT — соединение на каждый запрос" },
          cfg.pool_max, cfg.idle_timeout_ms / 2);
}

pub fn enabled() -> bool { POOL.get().is_some() }

/// Область пула: `tunnels` | `all` (None — пул выключен).
pub fn scope() -> Option<&'static str> { POOL.get().map(|p| if p.dht { "all" } else { "tunnels" }) }

/// Статистика пула: (открыто соединений за всё время, запросов по уже открытому, открыто сейчас).
pub async fn stats() -> (u64, u64, usize) {
    match POOL.get() {
        Some(p) => (p.opened.load(Ordering::Relaxed), p.reused.load(Ordering::Relaxed), p.conns.lock().await.len()),
        None => (0, 0, 0),
    }
}

/// Отправить запрос и дождаться ответа с тем же `request_id`.
/// `Ok(None)` — ответа нет за `wait_ms` (вызывающий формирует своё сообщение о тайм-ауте);
/// ошибки соединения (`connect timeout to …`, `TLS handshake timeout to …`) — как у `dial_expect`.
pub async fn request(scope: Scope, addr: SocketAddr, expected: Option<&NodeId>, frame: Frame,
                     connect_ms: u64, read_ms: u64, wait_ms: u64) -> Result<Option<(Frame, Option<NodeId>)>> {
    let Some(pool) = POOL.get().filter(|p| scope == Scope::Tunnel || p.dht) else {
        // соединение на каждый запрос
        let mut s = super::dial_expect(addr, connect_ms, read_ms, expected).await?;
        let peer = s.peer_node_id().copied();
        s.send(&frame).await.map_err(|e| anyhow::anyhow!("{e}"))?;
        return match tokio::time::timeout(Duration::from_millis(wait_ms), s.recv()).await {
            Err(_) => Ok(None),
            Ok(Err(e)) => bail!("{addr}: {e}"),
            Ok(Ok(resp)) => Ok(Some((resp, peer))),
        };
    };
    let key: Key = (addr, expected.copied());
    for attempt in 0..2 {
        let (conn, fresh) = pool.get_or_open(key, connect_ms, read_ms).await?;
        let (tx, rx) = oneshot::channel();
        conn.waiters.lock().map(|mut w| w.insert(frame.request_id, tx)).ok();
        conn.last_ms.store(now_ms(), Ordering::Relaxed);
        if conn.tx.send(Some(frame.clone())).await.is_err() {
            conn.close("запись невозможна");
            if !fresh && attempt == 0 { continue; }
            bail!("соединение с {addr} закрыто");
        }
        match tokio::time::timeout(Duration::from_millis(wait_ms), rx).await {
            Ok(Ok(resp)) => {
                conn.last_ms.store(now_ms(), Ordering::Relaxed);
                return Ok(Some((resp, conn.peer)));
            }
            Ok(Err(_)) => {
                // соединение закрылось до ответа: по уже открытому — один повтор по новому
                if !fresh && attempt == 0 { debug!("conn#{} закрылось до ответа — повтор по новому", conn.id); continue; }
                bail!("соединение с {addr} закрыто до ответа");
            }
            Err(_) => {
                conn.waiters.lock().map(|mut w| w.remove(&frame.request_id)).ok();
                // нет ответа — соединение под подозрением («полуоткрытое»: сосед пропал без разрыва, NAT забыл
                // соединение). Закрываем, иначе каждый следующий запрос к соседу ждал бы тот же тайм-аут
                conn.close("нет ответа за отведённое время");
                return Ok(None);
            }
        }
    }
    bail!("соединение с {addr} закрыто до ответа")
}

/// Отправить кадр без ожидания ответа (медиакадры). С пулом — в очередь записи открытого соединения;
/// без пула — новое соединение на кадр (дорого — для звонков нужен пул).
pub async fn send_oneway(scope: Scope, addr: SocketAddr, expected: Option<&NodeId>, frame: Frame,
                         connect_ms: u64, read_ms: u64) -> Result<()> {
    let Some(pool) = POOL.get().filter(|p| scope == Scope::Tunnel || p.dht) else {
        let mut s = super::dial_expect(addr, connect_ms, read_ms, expected).await?;
        s.send(&frame).await.map_err(|e| anyhow::anyhow!("{e}"))?;
        return Ok(());
    };
    let key: Key = (addr, expected.copied());
    for attempt in 0..2 {
        let (conn, fresh) = pool.get_or_open(key, connect_ms, read_ms).await?;
        conn.last_ms.store(now_ms(), Ordering::Relaxed);
        // переполненная очередь записи — кадр опоздал бы всё равно: отбрасываем, а не ждём
        match conn.tx.try_send(Some(frame.clone())) {
            Ok(()) => return Ok(()),
            Err(mpsc::error::TrySendError::Full(_)) => bail!("очередь соединения с {addr} переполнена — медиакадр отброшен"),
            Err(mpsc::error::TrySendError::Closed(_)) => {
                conn.close("запись невозможна");
                if !fresh && attempt == 0 { continue; }
                bail!("соединение с {addr} закрыто");
            }
        }
    }
    bail!("соединение с {addr} закрыто")
}

impl Pool {
    async fn get_or_open(&'static self, key: Key, connect_ms: u64, read_ms: u64) -> Result<(Arc<Conn>, bool)> {
        {
            let mut m = self.conns.lock().await;
            if let Some(c) = m.get(&key) {
                if !c.closed.load(Ordering::SeqCst) {
                    self.reused.fetch_add(1, Ordering::Relaxed);
                    return Ok((c.clone(), false));
                }
                m.remove(&key);
            }
        }
        // рукопожатие — вне блокировки пула (не задерживать запросы к другим узлам)
        let stream = super::dial_expect(key.0, connect_ms, read_ms, key.1.as_ref()).await?;
        let (io, rest, peer) = stream.into_parts();
        let (rh, mut wh) = tokio::io::split(io);
        let (tx, mut rx) = mpsc::channel::<Option<Frame>>(256);
        let conn = Arc::new(Conn {
            id: self.next_id.fetch_add(1, Ordering::Relaxed), key, peer, tx,
            waiters: std::sync::Mutex::new(HashMap::new()), closed: AtomicBool::new(false), last_ms: AtomicU64::new(now_ms()),
        });
        // запись
        let c = conn.clone();
        tokio::spawn(async move {
            while let Some(Some(f)) = rx.recv().await {
                if let Err(e) = write_frame(&mut wh, &f).await { c.close(&format!("ошибка записи: {e}")); break; }
            }
            use tokio::io::AsyncWriteExt;
            let _ = wh.shutdown().await;
        });
        // чтение: ответы — ожидающим по request_id
        let c = conn.clone();
        let idle = self.idle_ms;
        let mut rd = rest.with(rh);
        tokio::spawn(async move {
            loop {
                match rd.next(idle).await {
                    Some(ReadItem::Frame(f)) => {
                        let w = c.waiters.lock().ok().and_then(|mut w| w.remove(&f.request_id));
                        match w { Some(tx) => { let _ = tx.send(f); } None => debug!("conn#{}: ответ без ожидающего", c.id) }
                    }
                    Some(ReadItem::Error(e)) => { c.close(&format!("ошибка протокола: {e}")); break; }
                    Some(ReadItem::Eof) => { c.close("сосед закрыл соединение"); break; }
                    None => {
                        let busy = c.waiters.lock().map(|w| !w.is_empty()).unwrap_or(false);
                        if !busy && now_ms().saturating_sub(c.last_ms.load(Ordering::Relaxed)) >= idle {
                            c.close("простой"); break;
                        }
                    }
                }
            }
            if let Some(p) = POOL.get() {
                let mut m = p.conns.lock().await;
                if m.get(&c.key).is_some_and(|x| x.id == c.id) { m.remove(&c.key); }
            }
        });
        self.opened.fetch_add(1, Ordering::Relaxed);
        let mut m = self.conns.lock().await;
        // пока шло рукопожатие, соединение к тому же узлу мог открыть другой запрос — берём его, своё закрываем
        if let Some(other) = m.get(&key) {
            if !other.closed.load(Ordering::SeqCst) {
                conn.close("дубликат");
                return Ok((other.clone(), false));
            }
        }
        if m.len() >= self.max {
            if let Some(old) = m.values().min_by_key(|c| c.last_ms.load(Ordering::Relaxed)).cloned() {
                old.close("пул переполнен");
                m.remove(&old.key);
            }
        }
        m.insert(key, conn.clone());
        debug!("conn#{} → {} открыто (пул: {})", conn.id, key.0, m.len());
        Ok((conn, true))
    }
}
