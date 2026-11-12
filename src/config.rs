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
    #[serde(default)]
    pub files:     FilesConfig,
    /// Только для экспериментов: внесение отказов на этом узле как ретрансляторе.
    #[serde(default)]
    pub fault:     FaultConfig,
    /// Офлайн-доставка через «почтовые ящики» в DHT.
    #[serde(default)]
    pub mailbox:   MailboxConfig,
    /// История на диске: срок хранения.
    #[serde(default)]
    pub history:   HistoryConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryConfig {
    /// Записи старше — клиенты предлагают удалить, перешифровать и оставить или вынести в открытый файл.
    pub retention_days: u64,
}

impl Default for HistoryConfig {
    fn default() -> Self { Self { retention_days: 365 } }
}

/// Офлайн-доставка: сообщение, не доставленное через туннель, отправитель кладёт в подписанную
/// запись DHT `mbox:<получатель>:<отправитель>`; получатель периодически забирает её сам
/// (исходящими запросами — работает и за NAT) и подтверждает APP_ACK.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct MailboxConfig {
    pub enabled: bool,
    /// Как часто получатель проверяет ящики известных собеседников.
    pub poll_interval_s: u64,
    /// Сообщения старше этого срока не ставятся в очередь и не принимаются из ящика.
    pub max_age_s: u64,
    /// Не больше стольких сообщений в одном ящике (старые вытесняются).
    pub max_messages: usize,
}

impl Default for MailboxConfig {
    fn default() -> Self { Self { enabled: true, poll_interval_s: 60, max_age_s: 7 * 24 * 3600, max_messages: 20 } }
}

/// Внесение отказов (эксперимент «стратегии выбора ретрансляторов»): ретранслятор с вероятностью `drop`
/// отвечает ошибкой вместо пересылки и задерживает каждую пересылку на `delay_ms`. По умолчанию выключено.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct FaultConfig {
    pub drop:     f64,
    pub delay_ms: u64,
}

// ── Files (§13.2) ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct FilesConfig {
    /// Максимальный размер принимаемого и отправляемого файла.
    pub max_bytes: u64,
    /// Размер блока (≤ 44 КиБ: блок + подпись + конверт укладываются в сегмент туннеля 48 КиБ).
    pub block_size: usize,
    /// Окно: столько блоков отправляется до подтверждения.
    pub window: usize,
    /// Неполный файл удаляется, если блоков не было дольше этого времени.
    pub incomplete_timeout_s: u64,
    /// Согласие на приём: `known` — сам принимается только файл до `auto_accept_bytes` от знакомого узла (ему писали
    /// сами или он в контактах клиента), остальное ждёт решения пользователя; `ask` — всегда спрашивать; `auto` — без
    /// вопросов (стенд, эксперименты). Квоты действуют всегда.
    #[serde(default = "default_file_accept")]
    pub accept: String,
    #[serde(default = "default_auto_accept_bytes")]
    pub auto_accept_bytes: u64,
    /// Сколько места могут занять полученные файлы вместе с принимаемыми.
    #[serde(default = "default_max_total_bytes")]
    pub max_total_bytes: u64,
    /// Сколько места на диске должно остаться свободным после приёма.
    #[serde(default = "default_min_free_bytes")]
    pub min_free_bytes: u64,
    /// Предложение без ответа пользователя отклоняется через это время.
    #[serde(default = "default_offer_timeout_s")]
    pub offer_timeout_s: u64,
}

fn default_file_accept() -> String { "known".into() }
fn default_auto_accept_bytes() -> u64 { 16 * 1024 * 1024 }
fn default_max_total_bytes() -> u64 { 2 * 1024 * 1024 * 1024 }
fn default_min_free_bytes() -> u64 { 512 * 1024 * 1024 }
fn default_offer_timeout_s() -> u64 { 120 }

impl Default for FilesConfig {
    fn default() -> Self {
        Self { max_bytes: 8 * 1024 * 1024 * 1024, block_size: 40 * 1024, window: 8, incomplete_timeout_s: 60,
               accept: default_file_accept(), auto_accept_bytes: default_auto_accept_bytes(),
               max_total_bytes: default_max_total_bytes(), min_free_bytes: default_min_free_bytes(),
               offer_timeout_s: default_offer_timeout_s() }
    }
}

// ── DHT ──────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DhtConfig {
    pub node_id_bits:              u16,
    pub k_bucket_size:             usize,
    pub alpha:                     usize,
    pub replication_factor:        usize,
    /// §10.1: STORE считается успешным при подтверждении не менее `store_quorum` реплик из R.
    #[serde(default = "default_store_quorum")]
    pub store_quorum:              usize,
    pub ttl_seconds:               u64,
    pub republish_interval_seconds: u64,
    pub refresh_interval_seconds:  u64,
    /// PING_TIMEOUT_MS: тайм-аут PONG (§5 ТЗ)
    pub ping_timeout_ms:           u64,
    /// READ_TIMEOUT_MS: тайм-аут ожидания RPC-ответа (§5)
    pub rpc_timeout_ms:            u64,
}

fn default_store_quorum() -> usize { 2 }

impl Default for DhtConfig {
    fn default() -> Self {
        Self {
            node_id_bits:               256,
            k_bucket_size:              3,
            alpha:                      3,
            replication_factor:         3,
            store_quorum:               2,
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
    /// READ_TIMEOUT_MS (§5): ожидание ответа на запрос и дочитывание начатого кадра.
    pub read_timeout_ms:        u64,
    /// Переиспользовать соединения между узлами (ТЗ §6 допускает оба подхода): одно TLS/AKE-соединение к соседу
    /// на много запросов (что именно — `pool_scope`). По умолчанию с 11.11.2026 — `true` (ADR-41);
    /// `false` — новое соединение на каждый запрос, как раньше.
    #[serde(default = "default_true")]
    pub reuse_connections:      bool,
    /// Простой соединения без начатого кадра: входящее закрывается через `idle_timeout_ms`,
    /// исходящее из пула — через половину этого времени (раньше сервера, чтобы не писать в закрытое).
    #[serde(default = "default_idle_ms")]
    pub idle_timeout_ms:        u64,
    /// Не больше стольких исходящих соединений в пуле (старые по последнему использованию закрываются).
    #[serde(default = "default_pool_max")]
    pub pool_max:               usize,
    /// Что идёт через пул при `reuse_connections`: `tunnels` — только трафик туннелей (TUNNEL_BUILD, TUNNEL_DATA)
    /// к выбранным ретрансляторам, DHT (PING, FIND_NODE, STORE, FIND_VALUE) — по-прежнему соединение на запрос;
    /// `all` — всё через пул.
    #[serde(default = "default_pool_scope")]
    pub pool_scope:             String,
    /// Что сообщать другим узлам в ошибках (кадр ERROR): `detailed` — адреса и причины (отладка, стенд);
    /// `generic` — без адресов и причин, следующий узел назван NodeID (иначе через ретранслятор можно выяснять,
    /// какие адреса и порты открыты в его сети); `auto` — detailed в debug-сборке, generic в release.
    #[serde(default = "default_peer_errors")]
    pub peer_errors:            String,
}

fn default_pool_scope() -> String { "tunnels".into() }
fn default_peer_errors() -> String { "auto".into() }

impl TransportConfig {
    /// Подробные ошибки для других узлов?
    pub fn detailed_peer_errors(&self) -> bool {
        match self.peer_errors.as_str() { "detailed" => true, "generic" => false, _ => cfg!(debug_assertions) }
    }
}

fn default_idle_ms() -> u64 { 60_000 }
fn default_pool_max() -> usize { 128 }

impl Default for TransportConfig {
    fn default() -> Self {
        Self {
            max_frame_payload:  65536,
            protocol_version:   1,
            connect_timeout_ms: 3000,
            read_timeout_ms:    5000,
            reuse_connections:  true,
            idle_timeout_ms:    default_idle_ms(),
            pool_max:           default_pool_max(),
            pool_scope:         default_pool_scope(),
            peer_errors:        default_peer_errors(),
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
    /// Выбор ретрансляторов: `closest` (по XOR к получателю), `random`, `rating` (по профилю) — §12, продвинутый уровень.
    #[serde(default = "default_selection")]
    pub relay_selection:    String,
    /// Быть ретранслятором чужих туннелей. `false` — «только клиент»: узел участвует в DHT,
    /// строит свои туннели и принимает адресованные ему, но не пересылает чужой трафик.
    #[serde(default = "default_true")]
    pub relay:              bool,
}

fn default_true() -> bool { true }

fn default_selection() -> String { "closest".into() }

impl Default for TunnelConfig {
    fn default() -> Self {
        Self {
            min_relays:       2,
            max_hops:         5,
            ttl_seconds:      600,
            pool_size:        1,
            build_timeout_ms: 15000,
            ack_timeout_ms:   5000,
            relay_selection:  default_selection(),
            relay:            true,
        }
    }
}

// ── Security ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityConfig {
    pub session_id_ttl_seconds: u64,
    /// Канал между узлами: `tls` (TLS 1.3, по умолчанию) или `ake` (собственное рукопожатие, §11.3).
    #[serde(default = "default_channel")]
    pub channel: String,
}

fn default_channel() -> String { "tls".into() }

impl Default for SecurityConfig {
    fn default() -> Self {
        Self { session_id_ttl_seconds: 3600, channel: default_channel() }
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
    /// Адрес, который узел сообщает пирам, если слушает на 0.0.0.0: IP в локальной сети
    /// («auto» — определить по маршруту по умолчанию). Пусто — 127.0.0.1 (все узлы на одной машине).
    /// Нужен, чтобы к узлу подключались другие устройства (телефон ↔ компьютер).
    #[serde(default)]
    pub advertise_host:  String,
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
            advertise_host:  String::new(),
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
