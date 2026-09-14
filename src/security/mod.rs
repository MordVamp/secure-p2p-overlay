//! security/mod.rs — Абстракция защиты канала.
//!
//! Упрощённый уровень:  TLS 1.3 с взаимной аутентификацией (tls.rs).
//! Продвинутый уровень: собственный AKE на X25519+Ed25519+HKDF (ake.rs — TODO).
//!
//! Точка расширения — трейт `ChannelSecurity`:
//!   impl TlsSecurity  (simplified) — `tls.rs`
//!   impl AkeSecurity  (advanced)  — `ake.rs` (добавить без правки остальных модулей)

pub mod tls;
pub mod session_tracker;

// TODO (продвинутый уровень): pub mod ake;
