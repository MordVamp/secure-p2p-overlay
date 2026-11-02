//! app/replay.rs — защита от повторов прикладных сообщений (ТЗ §11.4, §13.1).
//!
//! Сообщение принимается, если его подписанная метка времени попадает в окно (не старше `window`, не из будущего
//! дальше допуска на расхождение часов) и `message_id` в этом окне ещё не встречался. Принятые id
//! (с меткой времени) дописываются в `<state_dir>/replay.log` и загружаются при старте — повтор в пределах окна
//! отвергается и после перезапуска узла, а старше окна — по метке времени. Без файла (тесты) — только в памяти.

use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;

use crate::types::now_ms;

pub const WINDOW_MS: u64 = 10 * 60 * 1000;
pub const FUTURE_SKEW_MS: u64 = 2 * 60 * 1000;

pub struct ReplayGuard {
    seen:   HashMap<Vec<u8>, u64>,
    log:    Option<PathBuf>,
    window: u64,
}

impl ReplayGuard {
    pub fn in_memory() -> Self { Self { seen: HashMap::new(), log: None, window: WINDOW_MS } }

    /// С журналом на диске: загрузить id из окна, переписать журнал без устаревших записей.
    pub fn persistent(state_dir: &std::path::Path) -> Self {
        let log = state_dir.join("replay.log");
        let now = now_ms();
        let mut seen = HashMap::new();
        if let Ok(s) = std::fs::read_to_string(&log) {
            for line in s.lines() {
                let Some((id, ts)) = line.split_once(' ') else { continue };
                let (Ok(id), Ok(ts)) = (hex::decode(id), ts.parse::<u64>()) else { continue };
                if now.saturating_sub(ts) <= WINDOW_MS { seen.insert(id, ts); }
            }
        }
        let body: String = seen.iter().map(|(id, ts)| format!("{} {ts}\n", hex::encode(id))).collect();
        let _ = std::fs::create_dir_all(state_dir);
        let _ = std::fs::write(&log, body);
        Self { seen, log: Some(log), window: WINDOW_MS }
    }

    /// Проверить и запомнить. Err — сообщение вне окна или повтор.
    pub fn check(&mut self, message_id: &[u8], timestamp_ms: u64) -> Result<(), String> {
        let now = now_ms();
        if timestamp_ms + self.window < now { return Err("сообщение старше окна анти-replay".into()); }
        if timestamp_ms > now + FUTURE_SKEW_MS { return Err("метка времени сообщения из будущего".into()); }
        if self.seen.contains_key(message_id) { return Err("повтор: message_id уже принят".into()); }
        if self.seen.len() > 50_000 { let w = self.window; self.seen.retain(|_, ts| now.saturating_sub(*ts) <= w); }
        self.seen.insert(message_id.to_vec(), timestamp_ms);
        if let Some(p) = &self.log {
            if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(p) {
                let _ = writeln!(f, "{} {timestamp_ms}", hex::encode(message_id));
            }
        }
        Ok(())
    }
}
