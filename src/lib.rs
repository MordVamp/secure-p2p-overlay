//! p2p-overlay — защищённая оверлейная P2P-сеть (Kademlia DHT + TLS-туннели).
//! Уровень: Simplified (N=12-15, K=3, relays≥2)

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

// Фаза 5 ✅
pub mod bootstrap;

// Фаза 6 ✅
pub mod tunnel;

// Фаза 7-8: TODO
pub mod metrics;
// pub mod app;
