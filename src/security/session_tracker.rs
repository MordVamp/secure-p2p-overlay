//! security/session_tracker.rs — Отслеживание session_id для replay-защиты.
//!
//! Храним виденные request_id в sliding window по времени.
//! TTL окна берётся из конфига (session_id_ttl_seconds).

use std::collections::HashMap;
use std::sync::Mutex;
use crate::types::now_unix;

/// Потокобезопасный трекер request_id против replay-атаки.
pub struct SessionTracker {
    /// request_id (16 байт) → время первого наблюдения (Unix sec)
    seen: Mutex<HashMap<[u8; 16], u64>>,
    ttl:  u64,
}

impl SessionTracker {
    pub fn new(ttl_seconds: u64) -> Self {
        Self {
            seen: Mutex::new(HashMap::new()),
            ttl:  ttl_seconds,
        }
    }

    /// Вернуть `true` если request_id видится впервые (не replay).
    /// Сразу регистрирует его.
    pub fn check_and_register(&self, request_id: [u8; 16]) -> bool {
        let now = now_unix();
        let mut guard = self.seen.lock().unwrap();

        // Очистка устаревших
        guard.retain(|_, &mut t| now - t < self.ttl);

        if guard.contains_key(&request_id) {
            false // replay!
        } else {
            guard.insert(request_id, now);
            true
        }
    }
}
