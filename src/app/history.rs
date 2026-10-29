//! app/history.rs — Локальная история результатов отправки и приёма (§13.1).
//!
//! Одна JSON-строка на событие в `<state_dir>/history.jsonl` (append-only,
//! права 0600: файл содержит тексты сообщений). Переживает перезапуск узла.

use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use tracing::warn;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub ts_ms:      u64,
    /// "out" — отправка, "in" — приём.
    pub dir:        String,
    /// NodeID собеседника (hex).
    pub peer:       String,
    pub message_id: String,
    /// "delivered" (есть APP_ACK) | "sent" (без ожидания ACK) | "failed" | "received".
    pub status:     String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text:       Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rtt_ms:     Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error:      Option<String>,
    /// Разбивка доставки (только для delivered): lookup, построение туннеля (None — переиспользован),
    /// проход кадра по цепочке до получателя.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lookup_ms:  Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tunnel_build_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tunnel_send_ms:  Option<u64>,
}

pub struct History {
    path: PathBuf,
    lock: Mutex<()>,
}

impl History {
    pub fn open(state_dir: &Path) -> Self {
        Self { path: state_dir.join("history.jsonl"), lock: Mutex::new(()) }
    }

    pub fn path(&self) -> &Path { &self.path }

    /// Дописать событие. Ошибка записи не мешает доставке — только предупреждение.
    pub async fn append(&self, e: &HistoryEntry) {
        let _g = self.lock.lock().await;
        if let Err(err) = self.write_line(e) {
            warn!("history {}: {err}", self.path.display());
        }
    }

    fn write_line(&self, e: &HistoryEntry) -> std::io::Result<()> {
        if let Some(dir) = self.path.parent() { std::fs::create_dir_all(dir)?; }
        let mut opts = std::fs::OpenOptions::new();
        opts.create(true).append(true);
        #[cfg(unix)]
        { use std::os::unix::fs::OpenOptionsExt; opts.mode(0o600); }
        let mut f = opts.open(&self.path)?;
        let mut line = serde_json::to_vec(e)?;
        line.push(b'\n');
        f.write_all(&line)
    }

    /// Последние `limit` событий (старые → новые). Повреждённые строки пропускаются.
    pub async fn tail(&self, limit: usize) -> Vec<HistoryEntry> {
        let _g = self.lock.lock().await;
        let Ok(s) = std::fs::read_to_string(&self.path) else { return vec![] };
        let all: Vec<HistoryEntry> = s.lines()
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect();
        let skip = all.len().saturating_sub(limit);
        all.into_iter().skip(skip).collect()
    }
}
