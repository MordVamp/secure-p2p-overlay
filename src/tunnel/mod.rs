//! tunnel/ — Многопереходный оверлейный туннель (Фаза 6).
//!
//! Упрощённый уровень: min_relays=2, pool_size=1
//!
//! Состояния туннеля (машина состояний):
//!   BUILDING → ACTIVE → DEGRADED → CLOSING → DEAD
//!
//! Архитектура:
//!   TunnelBuilder  — согласует маршрут с relay-узлами (TUNNEL_BUILD RPC)
//!   TunnelManager  — пул туннелей, мониторинг, автовосстановление
//!   TunnelHop      — один промежуточный узел в маршруте
//!   TunnelSession  — активный туннель с состоянием + канал для данных

pub mod state;
pub mod hop;
pub mod builder;
pub mod manager;
pub mod session;

pub use state::TunnelState;
pub use manager::TunnelManager;
pub use session::TunnelSession;
