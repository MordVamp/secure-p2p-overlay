//! p2p-overlay — защищённая оверлейная P2P-сеть (Kademlia DHT + TLS).
//!
//! Уровень: Simplified (N=12-15, K=3, relays≥2)
//! Расширяемо до Advanced без рефакторинга (traits + config)

// Фаза 0
pub mod config;
pub mod types;

// Фаза 1 ✅
pub mod transport;
pub mod protocol;

// Фаза 2 ✅
pub mod identity;

// Фаза 3 ✅
pub mod security;

// Фаза 4 ✅
pub mod dht;
pub mod rpc;
pub mod node;

// Фаза 5 — Bootstrap
pub mod bootstrap;

// Фазы 6-10: TODO
// pub mod tunnel;
// pub mod app;
// pub mod metrics;
