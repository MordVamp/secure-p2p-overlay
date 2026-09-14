//! config.rs — Конфигурация узла, загружается из YAML.
//! Все изменяемые параметры вынесены сюда; правка конфига не требует
//! изменения исходного кода.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Выбор уровня реализации.  
/// `Simplified` — упрощённый (TLS 1.3, 2 ретранслятора, пул=1).  
/// `Advanced`   — продвинутый (AKE, ≥3 ретранслятора, пул≥3, файлы).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum NodeLevel {
    Simplified,
    Advanced,
}

impl Default for NodeLevel {
    fn default() -> Self { Self::Simplified }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DhtConfig {
    /// Бит в NodeID (SHA-256 → 256).
    #[serde(default = "default_node_id_bits")]
    pub node_id_bits: usize,
    /// K_BUCKET_SIZE: 3 (упрощённый), 4 (продвинутый).
    #[serde(default = "default_k")]
    pub k_bucket_size: usize,
    /// Параллельность lookup.
    #[serde(default = "default_alpha")]
    pub alpha: usize,
    /// Фактор репликации.
    #[serde(default = "default_r")]
    pub replication_factor: usize,
    /// TTL записей DHT (секунды).
    #[serde(default = "default_ttl")]
    pub ttl_seconds: u64,
    /// Интервал переопубликования (≤ ttl/2).
    #[serde(default = "default_republish")]
    pub republish_interval_seconds: u64,
    /// Интервал периодического refresh k-bucket.
    #[serde(default = "default_refresh")]
    pub refresh_interval_seconds: u64,
    /// Тайм-аут одного RPC (секунды).
    #[serde(default = "default_rpc_timeout")]
    pub rpc_timeout_seconds: u64,
}

impl Default for DhtConfig {
    fn default() -> Self {
        Self {
            node_id_bits: default_node_id_bits(),
            k_bucket_size: default_k(),
            alpha: default_alpha(),
            replication_factor: default_r(),
            ttl_seconds: default_ttl(),
            republish_interval_seconds: default_republish(),
            refresh_interval_seconds: default_refresh(),
            rpc_timeout_seconds: default_rpc_timeout(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransportConfig {
    /// Максимальный размер payload кадра.
    #[serde(default = "default_max_payload")]
    pub max_frame_payload: usize,
    /// Тайм-аут установки TCP-соединения (секунды).
    #[serde(default = "default_connect_timeout")]
    pub connect_timeout_seconds: u64,
    /// Тайм-аут чтения неполного кадра (секунды).
    #[serde(default = "default_read_timeout")]
    pub read_timeout_seconds: u64,
}

impl Default for TransportConfig {
    fn default() -> Self {
        Self {
            max_frame_payload: default_max_payload(),
            connect_timeout_seconds: default_connect_timeout(),
            read_timeout_seconds: default_read_timeout(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TunnelConfig {
    /// Минимальное число ретрансляторов (2 упрощённый / 3 продвинутый).
    #[serde(default = "default_min_relays")]
    pub min_relays: usize,
    /// Максимальная длина туннеля (hops).
    #[serde(default = "default_max_hops")]
    pub max_hops: usize,
    /// TTL туннеля (секунды).
    #[serde(default = "default_tunnel_ttl")]
    pub ttl_seconds: u64,
    /// Размер пула туннелей (1 упрощённый / ≥3 продвинутый).
    #[serde(default = "default_pool_size")]
    pub pool_size: usize,
    /// Тайм-аут построения туннеля (секунды).
    #[serde(default = "default_build_timeout")]
    pub build_timeout_seconds: u64,
    /// Тайм-аут ACK для обнаружения отказа (секунды).
    #[serde(default = "default_ack_timeout")]
    pub ack_timeout_seconds: u64,
}

impl Default for TunnelConfig {
    fn default() -> Self {
        Self {
            min_relays: default_min_relays(),
            max_hops: default_max_hops(),
            ttl_seconds: default_tunnel_ttl(),
            pool_size: default_pool_size(),
            build_timeout_seconds: default_build_timeout(),
            ack_timeout_seconds: default_ack_timeout(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityConfig {
    /// TTL session_id для replay-защиты (секунды).
    #[serde(default = "default_session_ttl")]
    pub session_id_ttl_seconds: u64,
}

impl Default for SecurityConfig {
    fn default() -> Self {
        Self { session_id_ttl_seconds: default_session_ttl() }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeConfig {
    /// Уровень реализации (simplified / advanced).
    #[serde(default)]
    pub level: NodeLevel,
    /// Каталог состояния (identity-ключи, хранилище).
    #[serde(default = "default_state_dir")]
    pub state_dir: PathBuf,
    /// TCP-порт узла.
    #[serde(default = "default_port")]
    pub listen_port: u16,
    /// Каталог для экспорта метрик (CSV/JSON).
    #[serde(default = "default_metrics_dir")]
    pub metrics_dir: PathBuf,
    #[serde(default)]
    pub dht: DhtConfig,
    #[serde(default)]
    pub transport: TransportConfig,
    #[serde(default)]
    pub tunnel: TunnelConfig,
    #[serde(default)]
    pub security: SecurityConfig,
}

impl Default for NodeConfig {
    fn default() -> Self {
        Self {
            level: NodeLevel::Simplified,
            state_dir: default_state_dir(),
            listen_port: default_port(),
            metrics_dir: default_metrics_dir(),
            dht: DhtConfig::default(),
            transport: TransportConfig::default(),
            tunnel: TunnelConfig::default(),
            security: SecurityConfig::default(),
        }
    }
}

impl NodeConfig {
    /// Загрузить конфиг из YAML-файла. Все поля с дефолтами необязательны.
    pub fn from_file(path: &std::path::Path) -> anyhow::Result<Self> {
        let content = std::fs::read_to_string(path)?;
        let cfg: Self = serde_yaml::from_str(&content)?;
        cfg.validate()?;
        Ok(cfg)
    }

    /// Валидация бизнес-правил ТЗ.
    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.dht.republish_interval_seconds <= self.dht.ttl_seconds / 2,
            "republish_interval_seconds must be ≤ ttl_seconds/2"
        );
        anyhow::ensure!(
            self.tunnel.min_relays >= 2,
            "min_relays must be ≥ 2 (simplified level)"
        );
        Ok(())
    }
}

// ── Дефолты ───────────────────────────────────────────────────────────────────

fn default_node_id_bits() -> usize { 256 }
fn default_k()            -> usize { 3 }
fn default_alpha()        -> usize { 3 }
fn default_r()            -> usize { 3 }
fn default_ttl()          -> u64   { 180 }
fn default_republish()    -> u64   { 80  }
fn default_refresh()      -> u64   { 60  }
fn default_rpc_timeout()  -> u64   { 5   }
fn default_max_payload()  -> usize { 65_536 }
fn default_connect_timeout() -> u64 { 10 }
fn default_read_timeout() -> u64   { 30 }
fn default_min_relays()   -> usize { 2  }
fn default_max_hops()     -> usize { 5  }
fn default_tunnel_ttl()   -> u64   { 600 }
fn default_pool_size()    -> usize { 1  }
fn default_build_timeout()-> u64   { 15 }
fn default_ack_timeout()  -> u64   { 5  }
fn default_session_ttl()  -> u64   { 3600 }
fn default_state_dir()    -> PathBuf { PathBuf::from("./state") }
fn default_metrics_dir()  -> PathBuf { PathBuf::from("./metrics") }
fn default_port()         -> u16   { 7000 }
