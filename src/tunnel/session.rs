//! tunnel/session.rs — Активная сессия туннеля.
//!
//! Контроль доступности: relay подтверждает TUNNEL_DATA только после ACK
//! следующего hop (rpc::handle_tunnel_data), поэтому TUNNEL_ACK от первого
//! relay означает, что кадр прошёл всю цепочку до получателя. Успешная
//! отправка обновляет `last_ack_ms`, ошибка переводит туннель в DEAD.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::{Mutex, mpsc, oneshot};
use tracing::{info, warn};
use uuid::Uuid;

use crate::config::NodeConfig;
use crate::tunnel::hop::TunnelHop;
use crate::tunnel::state::TunnelState;
use crate::types::now_ms;

/// Уникальный идентификатор туннеля.
pub type TunnelId = [u8; 16];

/// Данные для передачи через туннель.
#[derive(Debug)]
pub struct TunnelData {
    pub tunnel_id: TunnelId,
    pub payload:   Vec<u8>,
    pub sent_ms:   u64,
    /// Куда сообщить результат отправки (None — без ожидания).
    pub done:      Option<oneshot::Sender<anyhow::Result<()>>>,
}

/// Разделяемая часть сессии для задачи-отправителя: отметка ACK и отказ.
#[derive(Clone)]
pub struct SessionHealth {
    id:          TunnelId,
    state:       Arc<Mutex<TunnelState>>,
    last_ack_ms: Arc<AtomicU64>,
}

impl SessionHealth {
    /// Кадр подтверждён всей цепочкой: туннель жив.
    pub async fn mark_ack(&self) {
        self.last_ack_ms.store(now_ms(), Ordering::Relaxed);
        let mut s = self.state.lock().await;
        if *s == TunnelState::Degraded {
            info!("[tunnel-{}] {} → {} (ACK)", hex::encode(&self.id[..4]), *s, TunnelState::Active);
            *s = TunnelState::Active;
        }
    }

    /// Отправка не подтверждена: туннель непригоден, его заменит новый маршрут.
    pub async fn mark_dead(&self, reason: &str) {
        let mut s = self.state.lock().await;
        if s.can_transition_to(&TunnelState::Dead) {
            warn!("[tunnel-{}] {} → {} ({reason})", hex::encode(&self.id[..4]), *s, TunnelState::Dead);
            *s = TunnelState::Dead;
        }
    }

    pub async fn is_dead(&self) -> bool { *self.state.lock().await == TunnelState::Dead }
}

/// Активный туннель: маршрут + состояние + канал для данных.
pub struct TunnelSession {
    pub id:       TunnelId,
    pub hops:     Vec<TunnelHop>,
    state:        Arc<Mutex<TunnelState>>,
    pub cfg:      Arc<NodeConfig>,
    /// Канал для отправки данных через туннель.
    pub tx:       mpsc::Sender<TunnelData>,
    /// Время создания (unix ms).
    pub created_ms: u64,
    /// Последнее сквозное подтверждение (TUNNEL_ACK через всю цепочку или BUILD_OK).
    last_ack_ms:  Arc<AtomicU64>,
    /// B6: приёмник исходящей очереди. Раньше отбрасывался (`_rx`), и `send()`
    /// падал с "channel closed". Теперь живёт в сессии до `take_outbox()`.
    outbox:       Arc<Mutex<Option<mpsc::Receiver<TunnelData>>>>,
}

impl TunnelSession {
    pub fn new(hops: Vec<TunnelHop>, cfg: Arc<NodeConfig>)
        -> (Self, mpsc::Receiver<TunnelData>)
    {
        Self::with_id(Uuid::new_v4().into_bytes(), hops, cfg)
    }

    /// B3/B6: сессия с заданным tunnel_id (тем, что согласован с relay в TUNNEL_BUILD).
    pub fn with_id(id: TunnelId, hops: Vec<TunnelHop>, cfg: Arc<NodeConfig>)
        -> (Self, mpsc::Receiver<TunnelData>)
    {
        let (tx, rx) = mpsc::channel(64);
        let now = now_ms();
        let s = Self {
            id,
            hops,
            state:      Arc::new(Mutex::new(TunnelState::Building)),
            cfg,
            tx,
            created_ms: now,
            last_ack_ms: Arc::new(AtomicU64::new(now)),
            outbox:     Arc::new(Mutex::new(None)),
        };
        (s, rx)
    }

    /// Сохранить приёмник внутри сессии (чтобы канал не закрылся).
    pub async fn attach_outbox(&self, rx: mpsc::Receiver<TunnelData>) {
        *self.outbox.lock().await = Some(rx);
    }

    /// Забрать приёмник исходящей очереди (один раз) для задачи-отправителя.
    pub async fn take_outbox(&self) -> Option<mpsc::Receiver<TunnelData>> {
        self.outbox.lock().await.take()
    }

    pub fn health(&self) -> SessionHealth {
        SessionHealth { id: self.id, state: self.state.clone(), last_ack_ms: self.last_ack_ms.clone() }
    }

    pub async fn state(&self) -> TunnelState {
        *self.state.lock().await
    }

    /// Перевести в новое состояние (с проверкой допустимости).
    pub async fn transition(&self, next: TunnelState) -> bool {
        let mut cur = self.state.lock().await;
        if cur.can_transition_to(&next) {
            info!("[tunnel-{}] {} → {}", hex::encode(&self.id[..4]), *cur, next);
            *cur = next;
            true
        } else {
            warn!("[tunnel-{}] illegal transition {} → {}", hex::encode(&self.id[..4]), *cur, next);
            false
        }
    }

    /// Отправить данные через туннель (не блокирует, результат не ждём).
    pub async fn send(&self, payload: Vec<u8>) -> anyhow::Result<()> {
        self.enqueue(payload, None).await
    }

    /// Отправить и дождаться TUNNEL_ACK (= кадр прошёл цепочку до получателя).
    pub async fn send_confirmed(&self, payload: Vec<u8>) -> anyhow::Result<oneshot::Receiver<anyhow::Result<()>>> {
        let (tx, rx) = oneshot::channel();
        self.enqueue(payload, Some(tx)).await?;
        Ok(rx)
    }

    async fn enqueue(&self, payload: Vec<u8>, done: Option<oneshot::Sender<anyhow::Result<()>>>) -> anyhow::Result<()> {
        let state = self.state().await;
        if !state.is_usable() {
            anyhow::bail!("tunnel not usable (state={state})");
        }
        if self.is_expired() {
            anyhow::bail!("tunnel TTL expired");
        }
        self.tx.send(TunnelData {
            tunnel_id: self.id,
            payload,
            sent_ms: now_ms(),
            done,
        }).await.map_err(|e| anyhow::anyhow!("tunnel send: {e}"))?;
        Ok(())
    }

    /// Окно, в течение которого последнее подтверждение считается свежим.
    fn liveness_window_ms(&self) -> u64 { self.cfg.tunnel.ack_timeout_ms.max(1) * 12 }

    pub fn last_ack_age_ms(&self) -> u64 {
        now_ms().saturating_sub(self.last_ack_ms.load(Ordering::Relaxed))
    }

    /// Маршрут на relay живёт `tunnel.ttl_seconds`: туннель заменяется заранее (на 90% TTL),
    /// иначе relay с истёкшим маршрутом примет кадр как адресованный себе.
    pub fn is_expired(&self) -> bool {
        self.uptime_ms() >= self.cfg.tunnel.ttl_seconds.saturating_mul(900)
    }

    /// Число hop-ов, подтверждённых недавно (сквозной ACK подтверждает все hop сразу).
    pub fn alive_hops(&self) -> usize {
        if self.last_ack_age_ms() < self.liveness_window_ms() {
            self.hops.iter().filter(|h| h.confirmed).count()
        } else { 0 }
    }

    /// Периодическая проверка: TTL истёк → DEAD; давно нет подтверждений → DEGRADED
    /// (туннель остаётся пригодным: отказ всё равно обнаружится при следующей отправке).
    pub async fn refresh_state(&self) {
        let current = self.state().await;
        if current.is_usable() && self.is_expired() {
            self.transition(TunnelState::Dead).await;
            return;
        }
        let stale = self.last_ack_age_ms() >= self.liveness_window_ms();
        match current {
            TunnelState::Active if stale => { self.transition(TunnelState::Degraded).await; }
            TunnelState::Degraded if !stale => { self.transition(TunnelState::Active).await; }
            _ => {}
        }
    }

    pub fn id_hex(&self) -> String { hex::encode(&self.id[..4]) }

    pub fn uptime_ms(&self) -> u64 { now_ms() - self.created_ms }
}
