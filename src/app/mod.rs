//! app/ — Прикладной слой: E2E текстовые сообщения поверх туннеля (Фаза 7).
//!
//! Упрощённый уровень:
//!  - Текстовые сообщения (UTF-8, ≤ 48 KiB)
//!  - E2E шифрование: X25519 + HKDF + AES-256-GCM (security/e2e.rs), relay видят только конверт
//!  - Подпись Ed25519: sender подписывает (ts || to || text)
//!  - Доставка: find_node(target) → build_tunnel → send_data, APP_ACK
//!  - Локальная история отправки/приёма: `<state_dir>/history.jsonl` (history.rs)
//!
//! Продвинутый уровень (TODO): file_transfer, блоки ≤ 48 KiB, SHA-256 верификация

pub mod history;
pub mod message;
pub mod messenger;

pub use message::{AppMessage, MessageKind};
pub use messenger::Messenger;
