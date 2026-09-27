//! config.rs — Конфигурация узла P2P оверлейной сети.
//! Загружается из YAML-файла. Все параметры меняются без правки кода.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use anyhow::Result;

// ── Уровень реализации ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum NodeLevel {
    Simplified,
    Advanced,
}

impl Default for NodeLevel {
    fn default() -> Self { NodeLevel::Simplified }
}

// ── Корневая конфигурация ─────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct NodeConfig {
    #[serde(default)]
    pub level:     NodeLevel,
    #[serde(default)]
    pub dht:       DhtConfig,
    #[serde(default)]
    pub transport: TransportConfig,
    #[serde(default)]
    pub tunnel:    TunnelConfig,
    #[serde(default)]
    pub security:  SecurityConfig,
    #[serde(default)]
    pub node:      NodeParams,
}

// ── DHT ──────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DhtConfig {
    pub node_id_bits:              u16,
    pub k_bucket_size:             usize,
    pub alpha:                     usize,
    pub replication_factor:        usize,
    pub ttl_seconds:               u64,
    pub republish_interval_seconds: u64,
    pub refresh_interval_seconds:  u64,
    /// PING_TIMEOUT_MS: тайм-аут PONG (§5 ТЗ)
    pub ping_timeout_ms:           u64,
    /// READ_TIMEOUT_MS: тайм-аут ожидания RPC-ответа (§5)
    pub rpc_timeout_ms:            u64,
}

impl Default for DhtConfig {
    fn default() -> Self {
        Self {
            node_id_bits:               256,
            k_bucket_size:              3,
            alpha:                      3,
            replication_factor:         3,
            ttl_seconds:                180,
            republish_interval_seconds: 80,
            refresh_interval_seconds:   60,
            ping_timeout_ms:            5000,
            rpc_timeout_ms:             5000,
        }
    }
}

// ── Transport ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransportConfig {
    pub max_frame_payload:      u32,
    pub protocol_version:       u8,
    /// CONNECT_TIMEOUT_MS (§5)
    pub connect_timeout_ms:     u64,
    /// READ_TIMEOUT_MS (§5)
    pub read_timeout_ms:        u64,
}

impl Default for TransportConfig {
    fn default() -> Self {
        Self {
            max_frame_payload:  65536,
            protocol_version:   1,
            connect_timeout_ms: 3000,
            read_timeout_ms:    5000,
        }
    }
}

// ── Tunnel ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TunnelConfig {
    pub min_relays:         usize,
    pub max_hops:           usize,
    pub ttl_seconds:        u64,
    pub pool_size:          usize,
    pub build_timeout_ms:   u64,
    pub ack_timeout_ms:     u64,
}

impl Default for TunnelConfig {
    fn default() -> Self {
        Self {
            min_relays:       2,
            max_hops:         5,
            ttl_seconds:      600,
            pool_size:        1,
            build_timeout_ms: 15000,
            ack_timeout_ms:   5000,
        }
    }
}

// ── Security ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityConfig {
    pub session_id_ttl_seconds: u64,
}

impl Default for SecurityConfig {
    fn default() -> Self {
        Self { session_id_ttl_seconds: 3600 }
    }
}

// ── NodeParams ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeParams {
    pub state_dir:       PathBuf,
    /// LISTEN_HOST (§5): IP-адрес привязки TCP-сервера
    pub listen_host:     String,
    /// LISTEN_PORT (§5)
    pub listen_port:     u16,
    /// BOOTSTRAP_PEERS (§5): начальные контакты (может быть пустым у seed-узла)
    pub bootstrap_peers: Vec<String>,
    /// LOG_LEVEL (§5)
    pub log_level:       String,
    pub metrics_dir:     PathBuf,
}

impl Default for NodeParams {
    fn default() -> Self {
        Self {
            state_dir:       PathBuf::from("./state"),
            listen_host:     "0.0.0.0".to_string(),
            listen_port:     7000,
            bootstrap_peers: vec![],
            log_level:       "INFO".to_string(),
            metrics_dir:     PathBuf::from("./metrics"),
        }
    }
}

// ── Загрузка ──────────────────────────────────────────────────────────────────

impl NodeConfig {
    pub fn from_file(path: &Path) -> Result<Self> {
        let s = std::fs::read_to_string(path)?;
        Ok(serde_yaml::from_str(&s)?)
    }
}
