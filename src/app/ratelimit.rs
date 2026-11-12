//! app/ratelimit.rs — ведро токенов на отправителя: защита от спама сообщениями, вызовами и предложениями файлов.

use std::collections::HashMap;
use std::hash::Hash;
use std::sync::Mutex;

pub struct RateLimiter<K> {
    rate_per_s: f64,
    burst:      f64,
    map:        Mutex<HashMap<K, (f64, u64)>>,
}

impl<K: Eq + Hash + Copy> RateLimiter<K> {
    pub fn new(rate_per_s: f64, burst: f64) -> Self { Self { rate_per_s, burst, map: Mutex::new(HashMap::new()) } }

    /// true — можно принять (токен списан). `now_ms` — текущее время.
    pub fn allow(&self, key: K, now_ms: u64) -> bool {
        let mut m = self.map.lock().unwrap_or_else(|e| e.into_inner());
        if m.len() > 10_000 {
            // давно молчащие отправители давно накопили полное ведро — их можно забыть
            let (rate, burst) = (self.rate_per_s, self.burst);
            m.retain(|_, (t, last)| *t + now_ms.saturating_sub(*last) as f64 / 1000.0 * rate < burst);
        }
        let (tokens, last) = m.entry(key).or_insert((self.burst, now_ms));
        *tokens = (*tokens + now_ms.saturating_sub(*last) as f64 / 1000.0 * self.rate_per_s).min(self.burst);
        *last = now_ms;
        if *tokens < 1.0 { return false; }
        *tokens -= 1.0;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn burst_then_rate() {
        let l = RateLimiter::new(5.0, 30.0);
        let ok = (0..40).filter(|_| l.allow(1u8, 1000)).count();
        assert_eq!(ok, 30, "запас 30 сразу");
        assert!(!l.allow(1, 1000));
        assert!(l.allow(1, 1200), "через 200 мс — один токен (5/с)");
        assert!(!l.allow(1, 1200));
        assert!(l.allow(2, 1200), "другой отправитель не задет");
    }
}
