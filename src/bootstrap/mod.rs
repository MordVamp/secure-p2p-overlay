//! bootstrap/ — Схемы начального подключения (§22.3 ТЗ).
//!
//! Трейт BootstrapScheme позволяет добавлять схемы без рефакторинга.
//! Упрощённый уровень: Star + Ring.
//! Продвинутый уровень: Tree, MultiSeed (TODO).

use std::net::SocketAddr;

/// Трейт-точка расширения bootstrap-схем.
pub trait BootstrapScheme: Send + Sync {
    /// Вернуть список начальных контактов для узла с индексом `node_index`.
    fn initial_peers(
        &self,
        node_index: usize,
        all_addrs: &[SocketAddr],
    ) -> Vec<SocketAddr>;

    fn name(&self) -> &str;
}

// ── Star bootstrap ────────────────────────────────────────────────────────────

/// Все узлы подключаются к одному центральному seed-узлу (index 0).
/// Seed-узел запускается первым без bootstrap-контактов.
pub struct StarBootstrap {
    pub center_index: usize,
}

impl Default for StarBootstrap {
    fn default() -> Self { Self { center_index: 0 } }
}

impl BootstrapScheme for StarBootstrap {
    fn initial_peers(&self, node_index: usize, all_addrs: &[SocketAddr]) -> Vec<SocketAddr> {
        if node_index == self.center_index || all_addrs.is_empty() {
            vec![] // Seed-узел — без bootstrap-контактов
        } else {
            vec![all_addrs[self.center_index % all_addrs.len()]]
        }
    }

    fn name(&self) -> &str { "star" }
}

// ── Ring bootstrap ────────────────────────────────────────────────────────────

/// node_i подключается к node_{(i-1) % N} — предыдущему в кольце.
/// После bootstrap все должны уметь работать без seed-узла (§14 Отказ bootstrap).
pub struct RingBootstrap;

impl BootstrapScheme for RingBootstrap {
    fn initial_peers(&self, node_index: usize, all_addrs: &[SocketAddr]) -> Vec<SocketAddr> {
        if all_addrs.len() < 2 {
            return vec![];
        }
        let prev = if node_index == 0 { all_addrs.len() - 1 } else { node_index - 1 };
        vec![all_addrs[prev]]
    }

    fn name(&self) -> &str { "ring" }
}

// TODO (продвинутый уровень):
// pub struct TreeBootstrap { ... }
// pub struct MultiSeedBootstrap { ... }
