//! app/calls.rs — голосовые звонки: сигнализация (шаг 3) и ключ звонка с шифрованием медиа (шаг 4).
//!
//! Сигнализация — обычные подписанные E2E-сообщения (`CallOffer`, `CallAnswer`, `CallReject`, `CallHangup`):
//! проверка отправителя, анти-replay и доставка через туннель — как у текста.
//!
//! Ключ звонка. Каждая сторона создаёт одноразовую пару X25519 и передаёт открытую часть в подписанном
//! Offer/Answer — подмена ключа ломает подпись сообщения (MITM невозможен без ключа узла). Общий секрет
//! DH(eph_caller, eph_callee) → HKDF-SHA256 (salt = call_id, info = метка ‖ NodeID обеих сторон ‖ оба эфемерных
//! ключа) → два ключа AES-256-GCM, по одному на направление. Одноразовые секреты живут только до ответа
//! (Zeroizing) — компрометация долговременного ключа узла не раскрывает записанный трафик звонка.
//!
//! Медиакадр (идёт в `TUNNEL_MEDIA`, без подтверждений):
//! `call_id (16) ‖ seq (8, BE) ‖ AES-256-GCM(key_dir, nonce = dir (1) ‖ 0 (3) ‖ seq (8), aad = call_id ‖ seq,
//!  plaintext = sent_us (8, BE) ‖ payload)`. Повтор и сильно опоздавшие кадры отсекает окно на 64 номера.

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use anyhow::{bail, Result};
use hkdf::Hkdf;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use x25519_dalek::{PublicKey, StaticSecret};
use zeroize::Zeroizing;

use crate::types::{now_ms, NodeId};

pub const CALL_ID_LEN: usize = 16;
/// Сколько ждать ответа на вызов.
pub const RING_TIMEOUT_MS: u64 = 30_000;
const HKDF_INFO: &[u8] = b"p2p-overlay call v1";

// ── сигнализация ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallOffer {
    pub call_id: Vec<u8>,
    /// Одноразовый открытый ключ X25519 звонящего.
    pub eph_pk:  Vec<u8>,
    /// Параметры медиа (для шага 5): кодек и длительность кадра.
    pub codec:   String,
    pub frame_ms: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallAnswer {
    pub call_id: Vec<u8>,
    pub eph_pk:  Vec<u8>,
}

/// Отказ (`busy`, `rejected`) или завершение (`hangup`, `no_answer`, `timeout`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallEnd {
    pub call_id: Vec<u8>,
    pub reason:  String,
}

// ── ключи и медиакадры ──────────────────────────────────────────────────────

/// Ключи звонка: по одному на направление.
pub struct CallKeys {
    tx: Aes256Gcm,
    rx: Aes256Gcm,
    tx_dir: u8,
}

impl CallKeys {
    /// Вывести ключи. `caller`/`callee` — NodeID сторон, `caller_pk`/`callee_pk` — их одноразовые ключи;
    /// `my_secret` — свой одноразовый секрет, `i_am_caller` — кто мы.
    pub fn derive(call_id: &[u8], caller: &NodeId, callee: &NodeId, caller_pk: &[u8; 32], callee_pk: &[u8; 32],
                  my_secret: &StaticSecret, i_am_caller: bool) -> Result<Self> {
        let peer_pk = PublicKey::from(if i_am_caller { *callee_pk } else { *caller_pk });
        let shared = my_secret.diffie_hellman(&peer_pk);
        if !shared.was_contributory() { bail!("звонок: слабый одноразовый ключ собеседника"); }
        let mut info = Vec::with_capacity(HKDF_INFO.len() + 128);
        info.extend_from_slice(HKDF_INFO);
        info.extend_from_slice(&caller.0);
        info.extend_from_slice(&callee.0);
        info.extend_from_slice(caller_pk);
        info.extend_from_slice(callee_pk);
        let hk = Hkdf::<Sha256>::new(Some(call_id), shared.as_bytes());
        let mut okm = Zeroizing::new([0u8; 64]);
        hk.expand(&info, okm.as_mut()).map_err(|_| anyhow::anyhow!("HKDF"))?;
        let c2e = Aes256Gcm::new_from_slice(&okm[..32]).map_err(|_| anyhow::anyhow!("ключ"))?;
        let e2c = Aes256Gcm::new_from_slice(&okm[32..]).map_err(|_| anyhow::anyhow!("ключ"))?;
        Ok(if i_am_caller { Self { tx: c2e, rx: e2c, tx_dir: 1 } } else { Self { tx: e2c, rx: c2e, tx_dir: 2 } })
    }

    fn nonce(dir: u8, seq: u64) -> [u8; 12] {
        let mut n = [0u8; 12];
        n[0] = dir;
        n[4..].copy_from_slice(&seq.to_be_bytes());
        n
    }

    /// Зашифровать медиакадр: `call_id ‖ seq ‖ шифртекст(sent_us ‖ payload)`.
    pub fn seal(&self, call_id: &[u8], seq: u64, sent_us: u64, payload: &[u8]) -> Result<Vec<u8>> {
        let mut aad = call_id.to_vec();
        aad.extend_from_slice(&seq.to_be_bytes());
        let mut pt = Zeroizing::new(Vec::with_capacity(8 + payload.len()));
        pt.extend_from_slice(&sent_us.to_be_bytes());
        pt.extend_from_slice(payload);
        let ct = self.tx.encrypt(Nonce::from_slice(&Self::nonce(self.tx_dir, seq)), Payload { msg: &pt, aad: &aad })
            .map_err(|_| anyhow::anyhow!("шифрование медиакадра"))?;
        let mut out = Vec::with_capacity(CALL_ID_LEN + 8 + ct.len());
        out.extend_from_slice(call_id);
        out.extend_from_slice(&seq.to_be_bytes());
        out.extend_from_slice(&ct);
        Ok(out)
    }

    /// Расшифровать кадр собеседника → (seq, sent_us, payload). Ошибка — не тот ключ или изменённый кадр.
    pub fn open(&self, packet: &[u8]) -> Result<(u64, u64, Vec<u8>)> {
        let (call_id, seq) = parse_header(packet)?;
        let mut aad = call_id.to_vec();
        aad.extend_from_slice(&seq.to_be_bytes());
        let rx_dir = if self.tx_dir == 1 { 2 } else { 1 };
        let pt = self.rx.decrypt(Nonce::from_slice(&Self::nonce(rx_dir, seq)),
                                 Payload { msg: &packet[CALL_ID_LEN + 8..], aad: &aad })
            .map_err(|_| anyhow::anyhow!("медиакадр не прошёл проверку (ключ звонка или изменён)"))?;
        if pt.len() < 8 { bail!("медиакадр короче метки времени"); }
        let sent_us = u64::from_be_bytes(pt[..8].try_into().unwrap());
        Ok((seq, sent_us, pt[8..].to_vec()))
    }
}

/// `call_id` и номер кадра из заголовка (до расшифровки — чтобы найти звонок).
pub fn parse_header(packet: &[u8]) -> Result<(&[u8], u64)> {
    if packet.len() < CALL_ID_LEN + 8 + 16 { bail!("медиакадр слишком короткий"); }
    let seq = u64::from_be_bytes(packet[CALL_ID_LEN..CALL_ID_LEN + 8].try_into().unwrap());
    Ok((&packet[..CALL_ID_LEN], seq))
}

/// Новая одноразовая пара X25519.
pub fn ephemeral() -> (StaticSecret, [u8; 32]) {
    let s = StaticSecret::random_from_rng(rand::rngs::OsRng);
    let p = PublicKey::from(&s).to_bytes();
    (s, p)
}

/// Окно против повторов (как в IPsec/DTLS): самый большой принятый номер и битовая карта 64 предыдущих.
#[derive(Debug, Default, Clone)]
pub struct ReplayWindow { top: u64, bits: u64, any: bool }

impl ReplayWindow {
    /// true — номер новый (и отмечается); false — повтор или старше окна.
    pub fn accept(&mut self, seq: u64) -> bool {
        if !self.any { self.any = true; self.top = seq; self.bits = 1; return true; }
        if seq > self.top {
            let shift = seq - self.top;
            self.bits = if shift >= 64 { 1 } else { (self.bits << shift) | 1 };
            self.top = seq;
            return true;
        }
        let back = self.top - seq;
        if back >= 64 { return false; }
        let mask = 1u64 << back;
        if self.bits & mask != 0 { return false; }
        self.bits |= mask;
        true
    }
}

// ── состояние звонка и статистика приёма ────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CallState { Ringing, Active, Ended }

/// Статистика принятого медиапотока (для замеров и будущего индикатора качества).
#[derive(Debug, Default, Clone, Serialize)]
pub struct RxStats {
    pub packets:   u64,
    pub bytes:     u64,
    pub max_seq:   Option<u64>,
    /// Кадры, отвергнутые окном (повтор или старше окна) или проверкой ключа.
    pub rejected:  u64,
    pub reordered: u64,
    /// RFC 3550 §6.4.1, мс.
    pub jitter_ms: f64,
    #[serde(skip)]
    last: Option<(u64, u64)>,   // (recv_us, sent_us) предыдущего кадра
    /// Задержка в одну сторону, мс (по меткам отправителя; осмысленна при общих часах стенда), последние 5000.
    #[serde(skip)]
    pub one_way_ms: std::collections::VecDeque<f64>,
}

impl RxStats {
    pub fn record(&mut self, seq: u64, sent_us: u64, recv_us: u64, bytes: usize) {
        self.packets += 1;
        self.bytes += bytes as u64;
        if self.max_seq.is_some_and(|m| seq < m) { self.reordered += 1; }
        self.max_seq = Some(self.max_seq.map_or(seq, |m| m.max(seq)));
        if let Some((pr, ps)) = self.last {
            let d = (recv_us as f64 - pr as f64) - (sent_us as f64 - ps as f64);
            self.jitter_ms += (d.abs() / 1000.0 - self.jitter_ms) / 16.0;
        }
        self.last = Some((recv_us, sent_us));
        if self.one_way_ms.len() >= 5000 { self.one_way_ms.pop_front(); }
        self.one_way_ms.push_back((recv_us as f64 - sent_us as f64) / 1000.0);
    }

    /// Потеряно = ожидалось (по самому большому номеру) − принято.
    pub fn lost(&self) -> u64 { self.max_seq.map_or(0, |m| (m + 1).saturating_sub(self.packets)) }
}

/// Один звонок.
pub struct Call {
    pub id:          Vec<u8>,
    pub peer:        NodeId,
    pub outgoing:    bool,
    pub state:       CallState,
    pub created_ms:  u64,
    pub answered_ms: Option<u64>,
    pub ended_ms:    Option<u64>,
    pub end_reason:  Option<String>,
    /// Свой одноразовый секрет — до вывода ключей.
    pub secret:      Option<StaticSecret>,
    pub my_pk:       [u8; 32],
    /// Одноразовый ключ звонящего (у вызываемого — из Offer).
    pub caller_pk:   Option<[u8; 32]>,
    pub keys:        Option<CallKeys>,
    pub tx_seq:      u64,
    pub rx:          RxStats,
    pub window:      ReplayWindow,
}

impl Call {
    pub fn new(id: Vec<u8>, peer: NodeId, outgoing: bool) -> Self {
        let (secret, my_pk) = ephemeral();
        Self { id, peer, outgoing, state: CallState::Ringing, created_ms: now_ms(), answered_ms: None, ended_ms: None,
               end_reason: None, secret: Some(secret), my_pk, caller_pk: None, keys: None, tx_seq: 0,
               rx: RxStats::default(), window: ReplayWindow::default() }
    }

    /// Звонок закончился: секрет и ключи стираются.
    pub fn end(&mut self, reason: &str) {
        if self.state == CallState::Ended { return; }
        self.state = CallState::Ended;
        self.ended_ms = Some(now_ms());
        self.end_reason = Some(reason.to_owned());
        self.secret = None;
        self.keys = None;
    }

    pub fn json(&self) -> serde_json::Value {
        let mut ow: Vec<f64> = self.rx.one_way_ms.iter().copied().collect();
        ow.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let pct = |p: f64| if ow.is_empty() { None } else { Some(ow[((ow.len() - 1) as f64 * p).round() as usize]) };
        serde_json::json!({
            "call_id": hex::encode(&self.id), "peer": self.peer.to_hex(), "outgoing": self.outgoing,
            "state": self.state, "created_ms": self.created_ms, "answered_ms": self.answered_ms,
            "ended_ms": self.ended_ms, "end_reason": self.end_reason,
            "setup_ms": self.answered_ms.map(|a| a.saturating_sub(self.created_ms)),
            "tx_packets": self.tx_seq,
            "rx": self.rx, "rx_lost": self.rx.lost(),
            "one_way_ms": { "n": ow.len(), "median": pct(0.5), "p95": pct(0.95), "max": ow.last() },
        })
    }
}
