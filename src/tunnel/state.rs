//! tunnel/state.rs — Машина состояний туннеля.

use serde::{Deserialize, Serialize};
use std::fmt;

/// Состояния жизненного цикла туннеля.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TunnelState {
    /// Идёт согласование маршрута с relay-узлами.
    Building,
    /// Туннель активен, все relay отвечают.
    Active,
    /// Один или более relay не отвечают — восстановление.
    Degraded,
    /// Начато корректное закрытие.
    Closing,
    /// Туннель мёртв, ресурсы освобождены.
    Dead,
}

impl TunnelState {
    pub fn is_usable(&self) -> bool {
        matches!(self, TunnelState::Active | TunnelState::Degraded)
    }

    pub fn is_terminal(&self) -> bool {
        matches!(self, TunnelState::Dead)
    }

    /// Допустимые переходы состояний.
    pub fn can_transition_to(&self, next: &TunnelState) -> bool {
        matches!(
            (self, next),
            (TunnelState::Building,  TunnelState::Active)
            | (TunnelState::Building,  TunnelState::Dead)
            | (TunnelState::Active,    TunnelState::Degraded)
            | (TunnelState::Active,    TunnelState::Closing)
            | (TunnelState::Active,    TunnelState::Dead)
            | (TunnelState::Degraded,  TunnelState::Active)
            | (TunnelState::Degraded,  TunnelState::Dead)
            | (TunnelState::Closing,   TunnelState::Dead)
        )
    }
}

impl fmt::Display for TunnelState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self)
    }
}
