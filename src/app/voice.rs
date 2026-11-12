//! app/voice.rs — звук звонка в узле (шаги 5–7 плана): кодек Opus, адаптивный джиттер-буфер, восстановление
//! потерь (FEC / PLC), смешивание собеседников группового звонка, локальный аудиоканал к клиенту, тестовый тон
//! и оценка качества по E-модели (ITU-T G.107).
//!
//! Почему кодек в узле, а не в клиенте: одна реализация на Rust для телефона и компьютера; узел и так знает
//! номера кадров, потери и задержку; клиенту (Kotlin / egui) остаётся только микрофон и динамик.
//!
//! Локальный аудиоканал — UDP на 127.0.0.1 (порт и токен выдаёт защищённый токеном `/api/call/audio`):
//!   клиент → узел: `P2A1` ‖ токен (16) ‖ PCM 320 отсчётов s16le (20 мс, 16 кГц, моно)
//!   узел → клиент: `P2A1` ‖ PCM 320 отсчётов (смесь всех собеседников)
//! Ответ уходит на каждый кадр клиента, поэтому темп воспроизведения задают часы звуковой карты клиента
//! (захват и воспроизведение на одном устройстве идут от одного генератора), а не таймер узла — нет дрейфа.

use std::collections::BTreeMap;

use anyhow::{bail, Result};
use serde::Serialize;

pub const RATE: u32 = 16_000;
/// Кадр 20 мс.
pub const FRAME: usize = 320;
pub const FRAME_MS: u32 = 20;
pub const CODEC: &str = "opus/16000/20";
/// Тип полезной нагрузки медиакадра (первый байт открытого текста после метки времени).
pub const PT_PROBE: u8 = 0;
pub const PT_OPUS: u8 = 1;
pub const MAGIC: &[u8; 4] = b"P2A1";
pub const TOKEN_LEN: usize = 16;
/// Opus: задержка кодера (lookahead) на 16 кГц, мс.
pub const CODEC_DELAY_MS: f64 = 6.5;
/// Буфер: от 2 до 5 кадров (40–100 мс).
const JB_MIN: usize = 2;
const JB_MAX: usize = 5;
/// Сколько кадров подряд маскировать, прежде чем считать поток прерванным и заново набирать буфер.
const JB_RESYNC: u32 = 10;

// ── кодер ───────────────────────────────────────────────────────────────────

/// Кодер исходящего звука: один на узел (кадр кодируется один раз, шифруется отдельно для каждого собеседника).
pub struct Encoder { enc: opus::Encoder, pub loss_hint: i32 }

impl Encoder {
    pub fn new() -> Result<Self> {
        let mut enc = opus::Encoder::new(RATE, opus::Channels::Mono, opus::Application::Voip)?;
        enc.set_bitrate(opus::Bitrate::Bits(24_000))?;
        // встроенная избыточность: в кадре N — грубая копия кадра N−1, получатель восстанавливает одиночную потерю
        enc.set_inband_fec(true)?;
        enc.set_packet_loss_perc(10)?;
        Ok(Self { enc, loss_hint: 10 })
    }

    /// Подстройка избыточности под потери, о которых сообщил собеседник (отчёт получателя).
    pub fn adapt(&mut self, loss_pct: f64) {
        let p = (loss_pct.ceil() as i32 + 5).clamp(5, 40);
        if p != self.loss_hint && self.enc.set_packet_loss_perc(p).is_ok() { self.loss_hint = p; }
    }

    pub fn encode(&mut self, pcm: &[i16]) -> Result<Vec<u8>> {
        if pcm.len() != FRAME { bail!("кадр должен быть {FRAME} отсчётов"); }
        Ok(self.enc.encode_vec(pcm, 400)?)
    }
}

// ── джиттер-буфер и декодер на одного собеседника ───────────────────────────

/// Что выдал буфер на очередной такт воспроизведения.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Playout { Frame, Fec, Concealed, Silence, /** растяжение: буфер растёт на кадр */ Stretch }

/// Статистика воспроизведения (для UI и сценария).
#[derive(Debug, Default, Clone, Serialize)]
pub struct PlayStats {
    pub played:    u64,
    /// Восстановлено по избыточности следующего кадра (FEC).
    pub fec:       u64,
    /// Замаскировано (кадра нет ни сам по себе, ни в FEC) — слышимая потеря.
    pub concealed: u64,
    /// Пришли после своего такта воспроизведения (для звука — потеряны).
    pub late:      u64,
    /// Вставлено кадров-растяжений (буфер углублялся после опозданий) — не потеря, но слышимая правка темпа.
    pub stretched: u64,
    /// Выброшены, чтобы догнать (буфер разросся после всплеска задержки).
    pub dropped:   u64,
    pub decode_errors: u64,
    /// Целевая глубина буфера сейчас и средняя фактическая, мс.
    pub target_ms: u32,
    pub buffer_ms: f64,
    /// Громкость принятого звука (0..1, RMS последнего кадра) — индикатор «говорит».
    pub level:     f32,
}

/// Адаптивный джиттер-буфер: глубина 2–5 кадров по оценке джиттера (RFC 3550), догон при разрастании,
/// повторный набор после долгого перерыва.
pub struct JitterBuffer {
    frames:  BTreeMap<u64, Vec<u8>>,
    next:    Option<u64>,
    started: bool,
    missing_run: u32,
    target:  usize,
    depth_sum: f64,
    depth_n: u64,
    /// Добавка к цели после опозданий (0..3 кадра) и такты без опозданий (для спада).
    late_boost: usize,
    calm_ticks: u32,
    since_stretch: u32,
}

impl Default for JitterBuffer {
    fn default() -> Self {
        Self { frames: BTreeMap::new(), next: None, started: false, missing_run: 0, target: JB_MIN, depth_sum: 0.0, depth_n: 0,
               late_boost: 0, calm_ticks: 0, since_stretch: 0 }
    }
}

impl JitterBuffer {
    /// Кадр из сети. false — опоздал (его такт уже прошёл).
    pub fn push(&mut self, seq: u64, data: Vec<u8>) -> bool {
        if self.next.is_some_and(|n| seq < n) {
            // опоздал — буфер мелковат для этого пути: углубить (как NetEq в WebRTC)
            self.late_boost = (self.late_boost + 1).min(3);
            self.calm_ticks = 0;
            return false;
        }
        self.frames.insert(seq, data);
        while self.frames.len() > 50 { self.frames.pop_first(); }
        true
    }

    /// Целевая глубина: 1 кадр + 3 джиттера + добавка после опозданий, в пределах 2–5 кадров.
    pub fn adapt(&mut self, jitter_ms: f64) {
        let t = 1 + (3.0 * jitter_ms / FRAME_MS as f64).ceil() as usize + self.late_boost;
        self.target = t.clamp(JB_MIN, JB_MAX);
    }

    pub fn target_ms(&self) -> u32 { self.target as u32 * FRAME_MS }
    pub fn avg_depth_ms(&self) -> f64 { if self.depth_n == 0 { 0.0 } else { self.depth_sum / self.depth_n as f64 } }

    /// Такт воспроизведения: (что выдать, данные для декодера).
    pub fn pop(&mut self, st: &mut PlayStats) -> (Playout, Option<Vec<u8>>) {
        if !self.started {
            if self.frames.len() < self.target { return (Playout::Silence, None); }
            self.started = true;
            self.next = self.frames.keys().next().copied();
            self.missing_run = 0;
        }
        // догон: в буфере заметно больше цели — выбросить самые старые
        while self.frames.len() > self.target + 3 {
            if let Some((s, _)) = self.frames.pop_first() { st.dropped += 1; self.next = Some(s + 1); }
        }
        // спад добавки: 5 с без опозданий
        self.calm_ticks += 1;
        self.since_stretch += 1;
        if self.calm_ticks >= 250 && self.late_boost > 0 { self.late_boost -= 1; self.calm_ticks = 0; }
        let n = self.next.unwrap_or(0);
        // буфер мельче цели — растянуть звук на кадр (не чаще раза в 100 мс), не сдвигая номер
        if self.frames.contains_key(&n) && self.frames.len() < self.target && self.since_stretch >= 5 {
            self.since_stretch = 0;
            return (Playout::Stretch, None);
        }
        self.next = Some(n + 1);
        self.depth_sum += (self.frames.len() as f64) * FRAME_MS as f64;
        self.depth_n += 1;
        if let Some(d) = self.frames.remove(&n) {
            self.missing_run = 0;
            return (Playout::Frame, Some(d));
        }
        self.missing_run += 1;
        if self.missing_run > JB_RESYNC && self.frames.is_empty() {
            // собеседник замолчал или поток прервался — набрать буфер заново
            self.started = false;
            return (Playout::Silence, None);
        }
        match self.frames.get(&(n + 1)) {
            Some(d) => (Playout::Fec, Some(d.clone())),
            None => (Playout::Concealed, None),
        }
    }
}

/// Декодер Opus, который можно держать в общей структуре звонков.
struct Dec(opus::Decoder);
// SAFETY: состояние декодера меняется только через `&mut self` (decode); через `&self` доступны лишь функции,
// читающие const-указатель, так что разделяемая ссылка не даёт гонки данных. `Send` реализован в самом крейте.
unsafe impl Sync for Dec {}

/// Приём звука от одного собеседника: буфер + декодер.
pub struct LegAudio { pub jb: JitterBuffer, dec: Dec, pub stats: PlayStats }

impl LegAudio {
    pub fn new() -> Result<Self> {
        Ok(Self { jb: JitterBuffer::default(), dec: Dec(opus::Decoder::new(RATE, opus::Channels::Mono)?), stats: PlayStats::default() })
    }

    pub fn push(&mut self, seq: u64, opus: Vec<u8>) {
        if !self.jb.push(seq, opus) { self.stats.late += 1; }
    }

    /// Очередной кадр 20 мс (тишина, пока буфер набирается).
    pub fn next_frame(&mut self, jitter_ms: f64, out: &mut [i16; FRAME]) {
        self.jb.adapt(jitter_ms);
        let (kind, data) = self.jb.pop(&mut self.stats);
        let r = match kind {
            Playout::Silence => { out.fill(0); Ok(FRAME) }
            Playout::Frame => self.dec.0.decode(data.as_deref().unwrap_or(&[]), out, false),
            // из избыточности следующего кадра — восстановить текущий
            Playout::Fec => self.dec.0.decode(data.as_deref().unwrap_or(&[]), out, true),
            // маскировка декодера продолжает звук предыдущего кадра — так и растягиваем
            Playout::Concealed | Playout::Stretch => self.dec.0.decode(&[], out, false),
        };
        match kind {
            Playout::Frame => self.stats.played += 1,
            Playout::Fec => self.stats.fec += 1,
            Playout::Concealed => self.stats.concealed += 1,
            Playout::Stretch => self.stats.stretched += 1,
            Playout::Silence => {}
        }
        if r.is_err() { self.stats.decode_errors += 1; out.fill(0); }
        self.stats.level = rms(out);
        self.stats.target_ms = self.jb.target_ms();
        self.stats.buffer_ms = self.jb.avg_depth_ms();
    }
}

/// Сложить кадры собеседников с насыщением.
pub fn mix_into(acc: &mut [i32; FRAME], frame: &[i16; FRAME]) {
    for (a, s) in acc.iter_mut().zip(frame) { *a += *s as i32; }
}

pub fn clip(acc: &[i32; FRAME]) -> [i16; FRAME] {
    let mut o = [0i16; FRAME];
    for (d, s) in o.iter_mut().zip(acc) { *d = (*s).clamp(i16::MIN as i32, i16::MAX as i32) as i16; }
    o
}

/// RMS кадра, 0..1.
pub fn rms(pcm: &[i16]) -> f32 {
    if pcm.is_empty() { return 0.0; }
    let s: f64 = pcm.iter().map(|&x| (x as f64 / 32768.0).powi(2)).sum();
    (s / pcm.len() as f64).sqrt() as f32
}

// ── локальный канал и тестовый источник ─────────────────────────────────────

/// Разобрать кадр клиента: проверка метки и токена (сравнение за постоянное время).
pub fn parse_client(buf: &[u8], token: &[u8; TOKEN_LEN]) -> Option<[i16; FRAME]> {
    if buf.len() != 4 + TOKEN_LEN + FRAME * 2 || &buf[..4] != MAGIC { return None; }
    let diff = buf[4..4 + TOKEN_LEN].iter().zip(token).fold(0u8, |d, (a, b)| d | (a ^ b));
    if diff != 0 { return None; }
    let mut pcm = [0i16; FRAME];
    for (i, s) in pcm.iter_mut().enumerate() {
        let o = 4 + TOKEN_LEN + i * 2;
        *s = i16::from_le_bytes([buf[o], buf[o + 1]]);
    }
    Some(pcm)
}

pub fn client_frame(pcm: &[i16; FRAME]) -> Vec<u8> {
    let mut v = Vec::with_capacity(4 + FRAME * 2);
    v.extend_from_slice(MAGIC);
    for s in pcm { v.extend_from_slice(&s.to_le_bytes()); }
    v
}

/// Синусоида для тестового источника: кадр номер `n`.
pub fn tone_frame(freq: f64, n: u64, amp: f64) -> [i16; FRAME] {
    let mut o = [0i16; FRAME];
    for (i, s) in o.iter_mut().enumerate() {
        let t = (n * FRAME as u64 + i as u64) as f64 / RATE as f64;
        *s = (amp * 32767.0 * (2.0 * std::f64::consts::PI * freq * t).sin()) as i16;
    }
    o
}

/// Мощность частоты `f` в сигнале (алгоритм Гёрцеля), нормированная на длину.
pub fn goertzel(x: &[i16], f: f64) -> f64 {
    let w = 2.0 * std::f64::consts::PI * f / RATE as f64;
    let c = 2.0 * w.cos();
    let (mut s1, mut s2) = (0.0f64, 0.0f64);
    for &v in x {
        let s0 = v as f64 / 32768.0 + c * s1 - s2;
        s2 = s1;
        s1 = s0;
    }
    (s1 * s1 + s2 * s2 - c * s1 * s2) / (x.len().max(1) as f64).powi(2)
}

/// Частоты, присутствующие в сигнале: из `candidates` — те, чья мощность не меньше `rel` от самой сильной.
pub fn tones_present(x: &[i16], candidates: &[f64], rel: f64) -> Vec<f64> {
    let p: Vec<f64> = candidates.iter().map(|&f| goertzel(x, f)).collect();
    let max = p.iter().cloned().fold(0.0, f64::max);
    if max <= 1e-9 { return vec![]; }
    candidates.iter().zip(&p).filter(|(_, &v)| v >= max * rel).map(|(&f, _)| f).collect()
}

// ── оценка качества: E-модель ITU-T G.107 (упрощённая) ───────────────────────

/// Допущения для Opus 16 кГц 24 кбит/с с FEC: Ie = 10, Bpl = 20 (у ITU нет табличных значений для Opus;
/// взяты консервативно, как для узкополосного кодека с маскировкой потерь).
pub const IE_OPUS: f64 = 10.0;
pub const BPL_OPUS: f64 = 20.0;

/// Задержка «рот — ухо», мс: сеть + буфер + кадр + задержка кодера.
pub fn mouth_to_ear_ms(one_way_ms: f64, buffer_ms: f64) -> f64 {
    one_way_ms + buffer_ms + FRAME_MS as f64 + CODEC_DELAY_MS
}

/// R-фактор и MOS по задержке «рот — ухо» `d` (мс) и доле слышимых потерь `ppl` (%).
pub fn emodel(d: f64, ppl: f64) -> (f64, f64) {
    let id = 0.024 * d + if d > 177.3 { 0.11 * (d - 177.3) } else { 0.0 };
    let ie_eff = IE_OPUS + (95.0 - IE_OPUS) * ppl / (ppl + BPL_OPUS);
    let r = (93.2 - id - ie_eff).clamp(0.0, 100.0);
    let mos = if r <= 0.0 { 1.0 } else { 1.0 + 0.035 * r + r * (r - 60.0) * (100.0 - r) * 7e-6 };
    (r, mos.clamp(1.0, 4.5))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opus_roundtrip_keeps_tone() {
        let mut e = Encoder::new().unwrap();
        let mut l = LegAudio::new().unwrap();
        let mut out = Vec::new();
        for n in 0..60 {
            l.push(n, e.encode(&tone_frame(440.0, n, 0.3)).unwrap());
            let mut f = [0i16; FRAME];
            l.next_frame(0.0, &mut f);
            out.extend_from_slice(&f);
        }
        assert_eq!(tones_present(&out[out.len() - 16000..], &[300.0, 440.0, 660.0, 1000.0], 0.2), vec![440.0]);
        assert!(l.stats.played >= 55);
    }

    #[test]
    fn single_loss_recovered_by_fec_and_burst_concealed() {
        let mut e = Encoder::new().unwrap();
        let mut l = LegAudio::new().unwrap();
        let mut f = [0i16; FRAME];
        // в реальном темпе: кадр приходит, такт воспроизведения отстаёт на глубину буфера
        for n in 0..40u64 {
            let p = e.encode(&tone_frame(440.0, n, 0.3)).unwrap();
            if n != 10 && !(20..23).contains(&n) { l.push(n, p); }
            if n >= 1 { l.next_frame(0.0, &mut f); }
        }
        assert_eq!(l.stats.fec, 2, "потеря 10 и последняя в серии 20–22 восстанавливаются по следующему кадру");
        assert_eq!(l.stats.concealed, 2, "21 и 22 маскируются");
    }

    #[test]
    fn jitter_buffer_reorders_and_counts_late() {
        let mut jb = JitterBuffer::default();
        let mut st = PlayStats::default();
        jb.push(1, vec![1]);
        jb.push(0, vec![0]);
        assert_eq!(jb.pop(&mut st), (Playout::Frame, Some(vec![0])));
        assert_eq!(jb.pop(&mut st), (Playout::Frame, Some(vec![1])));
        assert!(!jb.push(1, vec![1]), "такт уже прошёл");
        jb.adapt(25.0);
        assert_eq!(jb.target_ms(), 100, "большой джиттер — буфер глубже (до 5 кадров)");
    }

    #[test]
    fn late_frames_deepen_buffer_by_stretching() {
        let mut e = Encoder::new().unwrap();
        let mut l = LegAudio::new().unwrap();
        let mut f = [0i16; FRAME];
        let pk: Vec<Vec<u8>> = (0..300).map(|n| e.encode(&tone_frame(440.0, n, 0.3)).unwrap()).collect();
        // первые 100 кадров: каждый пятый приходит на 3 такта позже — опаздывает при глубине 2–3
        let mut held: Vec<(u64, u64)> = vec![];
        for t in 0..300u64 {
            if t % 5 == 0 && t < 100 { held.push((t + 3, t)); } else { l.push(t, pk[t as usize].clone()); }
            for &(when, seq) in held.iter().filter(|h| h.0 == t) { l.push(seq, pk[seq as usize].clone()); }
            if t >= 1 { l.next_frame(0.0, &mut f); }
        }
        assert!(l.stats.late > 0 && l.stats.stretched > 0, "{:?}", l.stats);
        assert!(l.jb.target_ms() > 40, "буфер углубился: {}", l.jb.target_ms());
        let late_before = l.stats.late;
        // дальше опозданий нет — новые кадры не теряются
        assert!(late_before <= 6, "после углубления опоздания прекращаются: {:?}", l.stats);
    }

    #[test]
    fn emodel_matches_reference_points() {
        let (r, mos) = emodel(50.0, 0.0);
        assert!((r - 82.0).abs() < 0.1 && mos > 4.0, "{r} {mos}");
        let (_, bad) = emodel(400.0, 10.0);
        assert!(bad < 3.0);
    }

    #[test]
    fn client_frame_requires_token() {
        let token = [7u8; TOKEN_LEN];
        let mut b = MAGIC.to_vec();
        b.extend_from_slice(&token);
        b.extend(std::iter::repeat(0u8).take(FRAME * 2));
        assert!(parse_client(&b, &token).is_some());
        b[5] ^= 1;
        assert!(parse_client(&b, &token).is_none());
    }
}
