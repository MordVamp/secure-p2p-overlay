//! voice.rs — звук звонка на компьютере: микрофон и динамик через cpal, обмен с узлом — PCM 16 кГц кадрами
//! по 20 мс через локальный аудиоканал узла (UDP 127.0.0.1, токен в каждом кадре; кодек, буфер и
//! шифрование — в узле, `src/app/voice.rs`). Частота звуковой карты (обычно 44,1 или 48 кГц) приводится
//! к 16 кГц и обратно линейной интерполяцией.

use std::collections::VecDeque;
use std::net::UdpSocket;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

const RATE: u32 = 16_000;
const FRAME: usize = 320;
const MAGIC: &[u8; 4] = b"P2A1";
/// Сколько принятого звука держать перед динамиком (больше — выбрасывать старое): 200 мс.
const MAX_QUEUE: usize = RATE as usize / 5;

/// Линейная передискретизация с сохранением состояния между кусками.
struct Linear { step: f64, t: f64, prev: f32 }

impl Linear {
    fn new(from: u32, to: u32) -> Self { Self { step: from as f64 / to as f64, t: 0.0, prev: 0.0 } }
    fn push(&mut self, input: &[f32], out: &mut Vec<f32>) {
        for &x in input {
            while self.t <= 1.0 {
                out.push(self.prev + (x - self.prev) * self.t as f32);
                self.t += self.step;
            }
            self.t -= 1.0;
            self.prev = x;
        }
    }
}

pub struct CallAudio {
    pub muted: Arc<AtomicBool>,
    level: Arc<AtomicU32>,
    stop: Arc<AtomicBool>,
    _streams: Vec<cpal::Stream>,
    /// Устройства (для подсказки в интерфейсе).
    pub devices: String,
}

impl CallAudio {
    pub fn start(port: u16, token: Vec<u8>) -> Result<Self, String> {
        let sock = UdpSocket::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
        sock.connect(("127.0.0.1", port)).map_err(|e| e.to_string())?;
        sock.set_read_timeout(Some(Duration::from_millis(200))).ok();
        let sock = Arc::new(sock);
        let muted = Arc::new(AtomicBool::new(false));
        let level = Arc::new(AtomicU32::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let queue: Arc<Mutex<VecDeque<f32>>> = Arc::new(Mutex::new(VecDeque::new()));
        let host = cpal::default_host();
        let mut streams = vec![];
        let mut names = vec![];

        // приём: смесь собеседников от узла → очередь динамика
        {
            let (sock, queue, stop) = (sock.clone(), queue.clone(), stop.clone());
            std::thread::spawn(move || {
                let mut buf = [0u8; 2048];
                while !stop.load(Ordering::Relaxed) {
                    let Ok(n) = sock.recv(&mut buf) else { continue };
                    if n != 4 + FRAME * 2 || &buf[..4] != MAGIC { continue; }
                    let mut q = queue.lock().unwrap();
                    for i in 0..FRAME {
                        q.push_back(i16::from_le_bytes([buf[4 + 2 * i], buf[5 + 2 * i]]) as f32 / 32768.0);
                    }
                    while q.len() > MAX_QUEUE { q.pop_front(); }
                }
            });
        }

        // динамик
        if let Some(dev) = host.default_output_device() {
            let cfg = dev.default_output_config().map_err(|e| e.to_string())?;
            names.push(format!("🔊 {}", dev.name().unwrap_or_default()));
            let fmt = cfg.sample_format();
            let config: cpal::StreamConfig = cfg.into();
            let ch = config.channels as usize;
            let mut rs = Linear::new(RATE, config.sample_rate.0);
            let mut pending: VecDeque<f32> = VecDeque::new();
            let q = queue.clone();
            let mut fill = move |n: usize| -> Vec<f32> {
                let mut tmp = Vec::with_capacity(FRAME * 4);
                while pending.len() < n {
                    let chunk: Vec<f32> = {
                        let mut q = q.lock().unwrap();
                        if q.len() >= FRAME { q.drain(..FRAME).collect() } else { vec![0.0; FRAME] }
                    };
                    tmp.clear();
                    rs.push(&chunk, &mut tmp);
                    pending.extend(tmp.iter().copied());
                }
                pending.drain(..n).collect()
            };
            let err = |e| eprintln!("звук: {e}");
            let stream = match fmt {
                cpal::SampleFormat::F32 => dev.build_output_stream(&config, move |d: &mut [f32], _| {
                    let s = fill(d.len() / ch);
                    for (f, v) in d.chunks_mut(ch).zip(s) { f.fill(v); }
                }, err, None),
                cpal::SampleFormat::I16 => dev.build_output_stream(&config, move |d: &mut [i16], _| {
                    let s = fill(d.len() / ch);
                    for (f, v) in d.chunks_mut(ch).zip(s) { f.fill((v.clamp(-1.0, 1.0) * 32767.0) as i16); }
                }, err, None),
                cpal::SampleFormat::U16 => dev.build_output_stream(&config, move |d: &mut [u16], _| {
                    let s = fill(d.len() / ch);
                    for (f, v) in d.chunks_mut(ch).zip(s) { f.fill(((v.clamp(-1.0, 1.0) + 1.0) * 32767.5) as u16); }
                }, err, None),
                f => return Err(format!("формат вывода {f:?} не поддержан")),
            }.map_err(|e| e.to_string())?;
            stream.play().map_err(|e| e.to_string())?;
            streams.push(stream);
        }

        // микрофон: кадр 20 мс → узлу (темп задают часы звуковой карты); без микрофона — тишина по таймеру
        let mut hdr = MAGIC.to_vec();
        hdr.extend_from_slice(&token);
        let send = {
            let (sock, muted, level) = (sock.clone(), muted.clone(), level.clone());
            move |frame: &[f32]| {
                let mut b = hdr.clone();
                let m = muted.load(Ordering::Relaxed);
                let mut sum = 0.0f32;
                for &v in frame {
                    let s = if m { 0.0 } else { v.clamp(-1.0, 1.0) };
                    sum += s * s;
                    b.extend_from_slice(&((s * 32767.0) as i16).to_le_bytes());
                }
                level.store((sum / FRAME as f32).sqrt().to_bits(), Ordering::Relaxed);
                let _ = sock.send(&b);
            }
        };
        match host.default_input_device().and_then(|d| d.default_input_config().ok().map(|c| (d, c))) {
            Some((dev, cfg)) => {
                names.push(format!("🎤 {}", dev.name().unwrap_or_default()));
                let fmt = cfg.sample_format();
                let config: cpal::StreamConfig = cfg.into();
                let ch = config.channels as usize;
                let mut rs = Linear::new(config.sample_rate.0, RATE);
                let mut acc: Vec<f32> = Vec::with_capacity(FRAME * 2);
                let send = send;
                let mut take = move |mono: Vec<f32>| {
                    rs.push(&mono, &mut acc);
                    while acc.len() >= FRAME {
                        send(&acc[..FRAME]);
                        acc.drain(..FRAME);
                    }
                };
                let err = |e| eprintln!("микрофон: {e}");
                let stream = match fmt {
                    cpal::SampleFormat::F32 => dev.build_input_stream(&config, move |d: &[f32], _| {
                        take(d.chunks(ch).map(|f| f.iter().sum::<f32>() / ch as f32).collect())
                    }, err, None),
                    cpal::SampleFormat::I16 => dev.build_input_stream(&config, move |d: &[i16], _| {
                        take(d.chunks(ch).map(|f| f.iter().map(|&x| x as f32 / 32768.0).sum::<f32>() / ch as f32).collect())
                    }, err, None),
                    cpal::SampleFormat::U16 => dev.build_input_stream(&config, move |d: &[u16], _| {
                        take(d.chunks(ch).map(|f| f.iter().map(|&x| x as f32 / 32768.0 - 1.0).sum::<f32>() / ch as f32).collect())
                    }, err, None),
                    f => return Err(format!("формат микрофона {f:?} не поддержан")),
                }.map_err(|e| e.to_string())?;
                stream.play().map_err(|e| e.to_string())?;
                streams.push(stream);
            }
            None => {
                names.push("🎤 нет микрофона — только приём".into());
                let stop = stop.clone();
                std::thread::spawn(move || {
                    let silence = [0.0f32; FRAME];
                    let t0 = std::time::Instant::now();
                    let mut n = 0u32;
                    while !stop.load(Ordering::Relaxed) {
                        send(&silence);
                        n += 1;
                        let next = t0 + Duration::from_millis(20 * n as u64);
                        if let Some(d) = next.checked_duration_since(std::time::Instant::now()) { std::thread::sleep(d); }
                    }
                });
            }
        }
        Ok(Self { muted, level, stop, _streams: streams, devices: names.join(" · ") })
    }

    /// Громкость микрофона 0..1.
    pub fn level(&self) -> f32 { f32::from_bits(self.level.load(Ordering::Relaxed)) }
}

impl Drop for CallAudio {
    fn drop(&mut self) { self.stop.store(true, Ordering::Relaxed); }
}
