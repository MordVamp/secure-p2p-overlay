//! app/ — Прикладной слой: E2E текстовые сообщения поверх туннеля (Фаза 7).
//!
//! Упрощённый уровень:
//!  - Текстовые сообщения (UTF-8, ≤ 48 KiB)
//!  - E2E шифрование: TLS поверх туннеля (защита от relay-узлов)
//!  - Подпись Ed25519: sender подписывает (ts || to || text)
//!  - Доставка: find_node(target) → build_tunnel → send_data
//!
//! Продвинутый уровень (TODO): file_transfer, блоки ≤ 48 KiB, SHA-256 верификация

pub mod message;
pub mod messenger;

pub use message::{AppMessage, MessageKind};
pub use messenger::Messenger;
