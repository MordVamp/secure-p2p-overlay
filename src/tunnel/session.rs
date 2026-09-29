//! tunnel/session.rs — Активная сессия туннеля.

use std::sync::Arc;
use tokio::sync::{Mutex, mpsc};
use tracing::{info, warn};
use uuid::Uuid;

use crate::config::NodeConfig;
use crate::tunnel::hop::TunnelHop;
use crate::tunnel::state::TunnelState;
use crate::types::now_ms;

/// Уникальный идентификатор туннеля.
pub type TunnelId = [u8; 16];

/// Данные для передачи через туннель.
#[derive(Debug, Clone)]
pub struct TunnelData {
    pub tunnel_id: TunnelId,
    pub payload:   Vec<u8>,
    pub sent_ms:   u64,
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
}

impl TunnelSession {
    pub fn new(hops: Vec<TunnelHop>, cfg: Arc<NodeConfig>)
        -> (Self, mpsc::Receiver<TunnelData>)
    {
        let (tx, rx) = mpsc::channel(64);
        let s = Self {
            id:         Uuid::new_v4().into_bytes(),
            hops,
            state:      Arc::new(Mutex::new(TunnelState::Building)),
            cfg,
            tx,
            created_ms: now_ms(),
        };
        (s, rx)
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

    /// Отправить данные через туннель (не блокирует).
    pub async fn send(&self, payload: Vec<u8>) -> anyhow::Result<()> {
        let state = self.state().await;
        if !state.is_usable() {
            anyhow::bail!("tunnel not usable (state={state})");
        }
        self.tx.send(TunnelData {
            tunnel_id: self.id,
            payload,
            sent_ms: now_ms(),
        }).await.map_err(|e| anyhow::anyhow!("tunnel send: {e}"))?;
        Ok(())
    }

    /// Число живых hop-ов (для оценки Degraded).
    pub fn alive_hops(&self) -> usize {
        let t = self.cfg.tunnel.ack_timeout_ms;
        self.hops.iter().filter(|h| h.is_alive(t)).count()
    }

    /// Обновить состояние на основе живости hop-ов.
    pub async fn refresh_state(&self) {
        let min = self.cfg.tunnel.min_relays;
        let alive = self.alive_hops();
        let current = self.state().await;
        match current {
            TunnelState::Active if alive < min => {
                self.transition(TunnelState::Degraded).await;
            }
            TunnelState::Degraded if alive >= min => {
                self.transition(TunnelState::Active).await;
            }
            TunnelState::Degraded if alive == 0 => {
                self.transition(TunnelState::Dead).await;
            }
            _ => {}
        }
    }

    pub fn id_hex(&self) -> String { hex::encode(&self.id[..4]) }

    pub fn uptime_ms(&self) -> u64 { now_ms() - self.created_ms }
}
