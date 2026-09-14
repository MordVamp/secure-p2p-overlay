//! overlay_node — защищённая оверлейная P2P-сеть на Kademlia DHT.
//!
//! # Структура модулей
//!
//! ```text
//! ├── config          — параметры узла (YAML), без правки кода
//! ├── types           — общие типы (NodeId, Contact, …)
//! ├── transport       — ✅ TCP-кадрирование + async I/O          [Фаза 1]
//! │   ├── framing     — Frame, FrameReader, MsgType, FrameFlags
//! │   └── connection  — FramedStream<S>
//! ├── identity        — ✅ Ed25519 идентичность, NodeRecord       [Фаза 2]
//! │   ├── node_identity — NodeIdentity, compute_node_id
//! │   └── record      — NodeRecord, подпись, валидация
//! ├── security        — ✅ TLS 1.3 mutual auth + replay tracker   [Фаза 3]
//! │   ├── tls         — TlsSecurity (упрощённый уровень)
//! │   │   // TODO ake.rs — кастомный AKE (продвинутый уровень)
//! │   └── session_tracker — защита от replay по request_id
//! ├── dht             — 🚧 Kademlia DHT                           [Фаза 4]
//! │   ├── routing_table — k-bucket, RoutingTable
//! │   ├── storage     — TODO: DHT-хранилище + TTL
//! │   ├── rpc         — TODO: PING, FIND_NODE, STORE, FIND_VALUE
//! │   ├── lookup      — TODO: итеративный lookup (α=3)
//! │   └── node        — TODO: фасад DHT (join/put/get)
//! ├── bootstrap       — TODO: схемы star, ring (+tree, multi-seed)[Фаза 5]
//! ├── tunnel          — TODO: туннельная передача                  [Фаза 6]
//! │   ├── tunnel      — состояния BUILDING→ACTIVE→DEAD
//! │   ├── relay       — логика ретранслятора
//! │   └── manager     — пул туннелей, перестроение при отказе
//! ├── app             — TODO: прикладные сервисы                   [Фаза 7]
//! │   ├── messenger   — текстовые сообщения E2E
//! │   └── // TODO file_transfer.rs (продвинутый уровень)
//! ├── metrics         — TODO: сбор и экспорт метрик (CSV/JSON)    [Фаза 8]
//! │   └── // TODO relay_profiler.rs (продвинутый уровень)
//! └── overlay_node    — TODO: публичный фасад системы             [Фаза 7]
//! ```

// ── Фаза 0: конфиг и типы ────────────────────────────────────────────────────
pub mod config;
pub mod types;

// ── Фаза 1: Транспорт ✅ ──────────────────────────────────────────────────────
pub mod transport;

// ── Фаза 2: Идентичность ✅ ───────────────────────────────────────────────────
pub mod identity;

// ── Фаза 3: Безопасность ✅ ───────────────────────────────────────────────────
pub mod security;

// ── Фаза 4: DHT 🚧 (routing_table готова, остальное — следующий коммит) ───────
pub mod dht;

// ── Фазы 5-10: TODO ───────────────────────────────────────────────────────────
// pub mod bootstrap;   // Фаза 5
// pub mod tunnel;      // Фаза 6
// pub mod app;         // Фаза 7
// pub mod metrics;     // Фаза 8
