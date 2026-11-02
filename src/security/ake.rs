//! security/ake.rs — собственное аутентифицированное установление сессии (ТЗ §11.3), вариант канала вместо TLS.
//!
//! Включается `security.channel: ake` (или `p2p-node --channel ake`); вся сеть должна использовать один вариант.
//! Только библиотечные примитивы (§22): Ed25519, X25519, HKDF-SHA256, AES-256-GCM.
//!
//! Рукопожатие (подписанный Диффи–Хеллман с подписью транскрипта, как в SIGMA/TLS 1.3):
//! ```text
//! I → R  Hello  {version, NodeID_I, IK_I, EK_I, nonce_I}
//! R → I  Reply  {version, NodeID_R, IK_R, EK_R, nonce_R, session_id, sig_R = Sign(IK_R, "…R" ‖ T)}
//! I → R  Finish {sig_I = Sign(IK_I, "…I" ‖ T)}
//! T = "p2p-ake-v1" ‖ version ‖ 0x01 ‖ 0x02 ‖ NodeID_I ‖ NodeID_R ‖ IK_I ‖ IK_R ‖ EK_I ‖ EK_R ‖ nonce_I ‖ nonce_R ‖ session_id
//! k_I→R ‖ k_R→I = HKDF-SHA256(salt = SHA-256(T), ikm = X25519(EK_I, EK_R), info = "p2p-ake-v1 keys")
//! ```
//! Каждая сторона проверяет NodeID = SHA-256(0x01 ‖ IK) и подпись пира над своим транскриптом: подмена эфемерного ключа,
//! nonce или session_id меняет транскрипт, и подпись не сходится. Эфемерные ключи и session_id — новые на каждое
//! соединение (прямая секретность). Записи: `len(u32) ‖ counter(u64) ‖ AES-256-GCM(k_dir, nonce = 0⁴ ‖ counter,
//! aad = "p2p-ake-rec" ‖ session_id ‖ dir ‖ counter)`; счётчик строго монотонный — повтор и перестановка отвергаются,
//! запись другой сессии не расшифровывается. Ключи сессии обнуляются при закрытии (`Zeroizing`).

use std::io;
use std::pin::Pin;
use std::task::{ready, Context, Poll};

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use anyhow::{anyhow, bail, Context as _, Result};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use hkdf::Hkdf;
use rand::{rngs::OsRng, RngCore};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf};
use x25519_dalek::{EphemeralSecret, PublicKey};
use zeroize::Zeroizing;

use crate::identity::node_identity::{compute_node_id, verify_node_id_bytes};
use crate::types::NodeId;

pub const AKE_VERSION: u8 = 1;
const CONTEXT: &[u8] = b"p2p-ake-v1";
const SIG_R: &[u8] = b"p2p-ake-v1 responder signature";
const SIG_I: &[u8] = b"p2p-ake-v1 initiator signature";
const KEYS_INFO: &[u8] = b"p2p-ake-v1 keys";
const REC_AAD: &[u8] = b"p2p-ake-rec";
const MAX_HANDSHAKE_MSG: usize = 4096;
/// Максимум открытого текста в одной записи.
pub const MAX_RECORD_PLAIN: usize = 16 * 1024;
const MAX_RECORD: usize = 8 + MAX_RECORD_PLAIN + 16;
const DIR_I2R: u8 = 0x01;
const DIR_R2I: u8 = 0x02;

/// I → R. Поля открыты: идентичность не скрывается (не цель упрощённой схемы).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hello { pub version: u8, pub node_id: Vec<u8>, pub identity_pk: Vec<u8>, pub eph_pk: Vec<u8>, pub nonce: Vec<u8> }

/// R → I.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Reply {
    pub version: u8, pub node_id: Vec<u8>, pub identity_pk: Vec<u8>, pub eph_pk: Vec<u8>, pub nonce: Vec<u8>,
    pub session_id: Vec<u8>, pub signature: Vec<u8>,
}

/// I → R.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Finish { pub signature: Vec<u8> }

/// Канонический транскрипт (порядок и длины полей фиксированы, см. PROTOCOL.md).
pub fn transcript(h: &Hello, r_node: &[u8], r_ik: &[u8], r_eph: &[u8], r_nonce: &[u8], session_id: &[u8]) -> Vec<u8> {
    let mut t = Vec::with_capacity(CONTEXT.len() + 3 + 32 * 8 + 16);
    t.extend_from_slice(CONTEXT);
    t.push(AKE_VERSION);
    t.push(0x01); // роль инициатора
    t.push(0x02); // роль ответчика
    for part in [&h.node_id[..], r_node, &h.identity_pk[..], r_ik, &h.eph_pk[..], r_eph, &h.nonce[..], r_nonce, session_id] {
        t.extend_from_slice(part);
    }
    t
}

fn check_len(name: &str, v: &[u8], n: usize) -> Result<()> {
    if v.len() != n { bail!("AKE: поле {name} — {} байт, ожидалось {n}", v.len()); }
    Ok(())
}

fn vk(bytes: &[u8]) -> Result<VerifyingKey> {
    let arr: [u8; 32] = bytes.try_into().map_err(|_| anyhow!("AKE: identity-ключ не 32 байта"))?;
    VerifyingKey::from_bytes(&arr).map_err(|e| anyhow!("AKE: неверный identity-ключ: {e}"))
}

fn verify_sig(ik: &[u8], label: &[u8], t: &[u8], sig: &[u8]) -> Result<()> {
    let sig: [u8; 64] = sig.try_into().map_err(|_| anyhow!("AKE: подпись не 64 байта"))?;
    let mut msg = label.to_vec();
    msg.extend_from_slice(t);
    vk(ik)?.verify(&msg, &Signature::from_bytes(&sig)).map_err(|_| anyhow!("AKE: подпись транскрипта неверна"))
}

/// Ключи направлений из общего секрета и транскрипта.
fn derive(shared: &[u8], t: &[u8]) -> Result<(Zeroizing<[u8; 32]>, Zeroizing<[u8; 32]>)> {
    let salt = Sha256::digest(t);
    let mut okm = Zeroizing::new([0u8; 64]);
    Hkdf::<Sha256>::new(Some(&salt), shared).expand(KEYS_INFO, &mut okm[..]).map_err(|_| anyhow!("AKE: hkdf"))?;
    let (mut a, mut b) = (Zeroizing::new([0u8; 32]), Zeroizing::new([0u8; 32]));
    a.copy_from_slice(&okm[..32]);
    b.copy_from_slice(&okm[32..]);
    Ok((a, b))
}

fn dh(own: EphemeralSecret, peer_eph: &[u8]) -> Result<Zeroizing<[u8; 32]>> {
    let peer: [u8; 32] = peer_eph.try_into().map_err(|_| anyhow!("AKE: эфемерный ключ не 32 байта"))?;
    let shared = own.diffie_hellman(&PublicKey::from(peer));
    if !shared.was_contributory() { bail!("AKE: слабый эфемерный ключ пира"); }
    Ok(Zeroizing::new(*shared.as_bytes()))
}

fn random<const N: usize>() -> [u8; N] {
    let mut b = [0u8; N];
    OsRng.fill_bytes(&mut b);
    b
}

fn frame_msg<T: Serialize>(m: &T) -> Result<Vec<u8>> {
    let body = crate::protocol::payload::encode(m).map_err(|e| anyhow!("AKE encode: {e}"))?;
    let mut msg = (body.len() as u32).to_be_bytes().to_vec();
    msg.extend_from_slice(&body);
    Ok(msg)
}

async fn send_msg<S: AsyncWrite + Unpin, T: Serialize>(s: &mut S, m: &T) -> Result<()> {
    // длина и тело — одной записью: две мелкие записи подряд ловят задержку Нейгла + delayed ACK (~40 мс)
    s.write_all(&frame_msg(m)?).await?;
    s.flush().await?;
    Ok(())
}

async fn recv_msg<S: AsyncRead + Unpin, T: for<'de> Deserialize<'de>>(s: &mut S) -> Result<T> {
    let mut len = [0u8; 4];
    s.read_exact(&mut len).await.context("AKE: соединение закрыто во время рукопожатия")?;
    let len = u32::from_be_bytes(len) as usize;
    if len > MAX_HANDSHAKE_MSG { bail!("AKE: сообщение рукопожатия {len} байт > {MAX_HANDSHAKE_MSG}"); }
    let mut body = vec![0u8; len];
    s.read_exact(&mut body).await?;
    rmp_serde::from_slice(&body).map_err(|e| anyhow!("AKE: некорректное сообщение рукопожатия: {e}"))
}

/// Провайдер AKE для одного узла (аналог `TlsSecurity`).
pub struct AkeSecurity {
    key:     SigningKey,
    ik:      [u8; 32],
    node_id: NodeId,
}

impl AkeSecurity {
    pub fn new(signing_key: &SigningKey) -> Self {
        let vk = signing_key.verifying_key();
        Self { key: signing_key.clone(), ik: vk.to_bytes(), node_id: compute_node_id(&vk) }
    }

    pub fn from_identity(id: &crate::identity::node_identity::NodeIdentity) -> Self { Self::new(id.signing_key()) }

    fn sign(&self, label: &[u8], t: &[u8]) -> Vec<u8> {
        let mut msg = label.to_vec();
        msg.extend_from_slice(t);
        self.key.sign(&msg).to_bytes().to_vec()
    }

    /// Инициатор. `expected` — NodeID, к которому мы подключаемся (если известен).
    pub async fn connect<S: AsyncRead + AsyncWrite + Unpin>(&self, mut s: S, expected: Option<&NodeId>) -> Result<(AkeStream<S>, NodeId)> {
        let eph = EphemeralSecret::random_from_rng(OsRng);
        let hello = Hello {
            version: AKE_VERSION, node_id: self.node_id.0.to_vec(), identity_pk: self.ik.to_vec(),
            eph_pk: PublicKey::from(&eph).to_bytes().to_vec(), nonce: random::<32>().to_vec(),
        };
        send_msg(&mut s, &hello).await?;
        let r: Reply = recv_msg(&mut s).await?;
        if r.version != AKE_VERSION { bail!("AKE: версия {} не поддерживается", r.version); }
        for (n, v, l) in [("node_id", &r.node_id, 32), ("identity_pk", &r.identity_pk, 32), ("eph_pk", &r.eph_pk, 32),
                          ("nonce", &r.nonce, 32), ("session_id", &r.session_id, 16)] { check_len(n, v, l)?; }
        let peer = NodeId(r.node_id[..].try_into().unwrap());
        if !verify_node_id_bytes(&peer, &r.identity_pk) { bail!("AKE: NodeID ответчика не соответствует его ключу"); }
        if peer == self.node_id { bail!("AKE: ответчик выдаёт себя за инициатора"); }
        if let Some(exp) = expected {
            if peer != *exp { bail!("AKE: NodeID ответчика {} вместо ожидаемого {}", peer.short(), exp.short()); }
        }
        let t = transcript(&hello, &r.node_id, &r.identity_pk, &r.eph_pk, &r.nonce, &r.session_id);
        verify_sig(&r.identity_pk, SIG_R, &t, &r.signature)?;
        let shared = dh(eph, &r.eph_pk)?;
        let (k_i2r, k_r2i) = derive(&shared[..], &t)?;
        let sid: [u8; 16] = r.session_id[..].try_into().unwrap();
        // Finish уходит одним сегментом с первой записью инициатора (как Finished + данные в TLS 1.3):
        // отдельный мелкий сегмент ждал бы ACK (Нейгл + delayed ACK ≈ 40 мс на соединение).
        let mut st = AkeStream::new(s, sid, k_i2r, k_r2i, DIR_I2R, DIR_R2I);
        st.wbuf = frame_msg(&Finish { signature: self.sign(SIG_I, &t) })?;
        Ok((st, peer))
    }

    /// Ответчик.
    pub async fn accept<S: AsyncRead + AsyncWrite + Unpin>(&self, mut s: S) -> Result<(AkeStream<S>, NodeId)> {
        let h: Hello = recv_msg(&mut s).await?;
        if h.version != AKE_VERSION { bail!("AKE: версия {} не поддерживается", h.version); }
        for (n, v, l) in [("node_id", &h.node_id, 32), ("identity_pk", &h.identity_pk, 32), ("eph_pk", &h.eph_pk, 32),
                          ("nonce", &h.nonce, 32)] { check_len(n, v, l)?; }
        let peer = NodeId(h.node_id[..].try_into().unwrap());
        if !verify_node_id_bytes(&peer, &h.identity_pk) { bail!("AKE: NodeID инициатора не соответствует его ключу"); }
        if peer == self.node_id { bail!("AKE: инициатор выдаёт себя за ответчика"); }
        let eph = EphemeralSecret::random_from_rng(OsRng);
        let eph_pk = PublicKey::from(&eph).to_bytes().to_vec();
        let nonce = random::<32>().to_vec();
        let session_id = random::<16>();
        let t = transcript(&h, &self.node_id.0, &self.ik, &eph_pk, &nonce, &session_id);
        let reply = Reply {
            version: AKE_VERSION, node_id: self.node_id.0.to_vec(), identity_pk: self.ik.to_vec(), eph_pk, nonce,
            session_id: session_id.to_vec(), signature: self.sign(SIG_R, &t),
        };
        send_msg(&mut s, &reply).await?;
        let f: Finish = recv_msg(&mut s).await?;
        verify_sig(&h.identity_pk, SIG_I, &t, &f.signature)?;
        let shared = dh(eph, &h.eph_pk)?;
        let (k_i2r, k_r2i) = derive(&shared[..], &t)?;
        Ok((AkeStream::new(s, session_id, k_r2i, k_i2r, DIR_R2I, DIR_I2R), peer))
    }
}

/// Защищённый поток после рукопожатия: записи AES-256-GCM с монотонным счётчиком.
pub struct AkeStream<S> {
    inner:      S,
    session_id: [u8; 16],
    send_key:   Zeroizing<[u8; 32]>,
    recv_key:   Zeroizing<[u8; 32]>,
    send_dir:   u8,
    recv_dir:   u8,
    send_ctr:   u64,
    recv_ctr:   u64,
    rbuf:       Vec<u8>,
    plain:      Zeroizing<Vec<u8>>,
    plain_pos:  usize,
    wbuf:       Vec<u8>,
    wpos:       usize,
}

fn aad(sid: &[u8; 16], dir: u8, ctr: u64) -> Vec<u8> {
    let mut a = REC_AAD.to_vec();
    a.extend_from_slice(sid);
    a.push(dir);
    a.extend_from_slice(&ctr.to_be_bytes());
    a
}

fn nonce(ctr: u64) -> [u8; 12] {
    let mut n = [0u8; 12];
    n[4..].copy_from_slice(&ctr.to_be_bytes());
    n
}

fn bad(msg: String) -> io::Error { io::Error::new(io::ErrorKind::InvalidData, msg) }

impl<S> AkeStream<S> {
    fn new(inner: S, session_id: [u8; 16], send_key: Zeroizing<[u8; 32]>, recv_key: Zeroizing<[u8; 32]>, send_dir: u8, recv_dir: u8) -> Self {
        Self { inner, session_id, send_key, recv_key, send_dir, recv_dir, send_ctr: 0, recv_ctr: 0,
               rbuf: Vec::new(), plain: Zeroizing::new(Vec::new()), plain_pos: 0, wbuf: Vec::new(), wpos: 0 }
    }

    pub fn session_id(&self) -> [u8; 16] { self.session_id }

    /// Зашифровать одну запись (для тестов — доступ к «проводу»).
    pub fn seal_record(&mut self, plain: &[u8]) -> io::Result<Vec<u8>> {
        if plain.len() > MAX_RECORD_PLAIN { return Err(bad("AKE: запись слишком велика".into())); }
        let ctr = self.send_ctr;
        self.send_ctr = ctr.checked_add(1).ok_or_else(|| bad("AKE: счётчик записей исчерпан".into()))?;
        let ct = Aes256Gcm::new_from_slice(&self.send_key[..]).map_err(|_| bad("AKE: ключ".into()))?
            .encrypt(Nonce::from_slice(&nonce(ctr)), Payload { msg: plain, aad: &aad(&self.session_id, self.send_dir, ctr) })
            .map_err(|_| bad("AKE: шифрование".into()))?;
        let mut rec = Vec::with_capacity(12 + ct.len());
        rec.extend_from_slice(&((8 + ct.len()) as u32).to_be_bytes());
        rec.extend_from_slice(&ctr.to_be_bytes());
        rec.extend_from_slice(&ct);
        Ok(rec)
    }

    /// Разобрать одну полную запись из буфера чтения. Повтор, перестановка, подмена — ошибка.
    fn open_record(&mut self) -> io::Result<bool> {
        if self.rbuf.len() < 4 { return Ok(false); }
        let len = u32::from_be_bytes(self.rbuf[..4].try_into().unwrap()) as usize;
        if !(8 + 16..=MAX_RECORD).contains(&len) { return Err(bad(format!("AKE: недопустимая длина записи {len}"))); }
        if self.rbuf.len() < 4 + len { return Ok(false); }
        let rec: Vec<u8> = self.rbuf.drain(..4 + len).skip(4).collect();
        let ctr = u64::from_be_bytes(rec[..8].try_into().unwrap());
        if ctr != self.recv_ctr {
            return Err(bad(format!("AKE: счётчик записи {ctr}, ожидался {} — повтор или перестановка", self.recv_ctr)));
        }
        let pt = Aes256Gcm::new_from_slice(&self.recv_key[..]).map_err(|_| bad("AKE: ключ".into()))?
            .decrypt(Nonce::from_slice(&nonce(ctr)), Payload { msg: &rec[8..], aad: &aad(&self.session_id, self.recv_dir, ctr) })
            .map_err(|_| bad("AKE: запись не прошла проверку подлинности (подмена или чужая сессия)".into()))?;
        self.recv_ctr += 1;
        self.plain = Zeroizing::new(pt);
        self.plain_pos = 0;
        Ok(true)
    }
}

impl<S: AsyncWrite + Unpin> AkeStream<S> {
    fn poll_drain(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        while self.wpos < self.wbuf.len() {
            let n = ready!(Pin::new(&mut self.inner).poll_write(cx, &self.wbuf[self.wpos..]))?;
            if n == 0 { return Poll::Ready(Err(io::ErrorKind::WriteZero.into())); }
            self.wpos += n;
        }
        self.wbuf.clear();
        self.wpos = 0;
        Poll::Ready(Ok(()))
    }
}

impl<S: AsyncRead + AsyncWrite + Unpin> AsyncRead for AkeStream<S> {
    fn poll_read(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        // отложенный Finish (или хвост записи) — до чтения, иначе инициатор, начавший с чтения, ждал бы вечно
        ready!(this.poll_drain(cx))?;
        loop {
            if this.plain_pos < this.plain.len() {
                let n = buf.remaining().min(this.plain.len() - this.plain_pos);
                buf.put_slice(&this.plain[this.plain_pos..this.plain_pos + n]);
                this.plain_pos += n;
                return Poll::Ready(Ok(()));
            }
            if this.open_record()? { continue; }
            let mut tmp = [0u8; 8192];
            let mut rb = ReadBuf::new(&mut tmp);
            ready!(Pin::new(&mut this.inner).poll_read(cx, &mut rb))?;
            if rb.filled().is_empty() {
                return if this.rbuf.is_empty() { Poll::Ready(Ok(())) }
                       else { Poll::Ready(Err(io::Error::new(io::ErrorKind::UnexpectedEof, "AKE: обрыв посреди записи"))) };
            }
            this.rbuf.extend_from_slice(rb.filled());
        }
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for AkeStream<S> {
    fn poll_write(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        // недописанное (в т.ч. Finish) склеивается с новой записью в один сегмент; буфер не растёт без предела
        if this.wbuf.len() - this.wpos > MAX_RECORD { ready!(this.poll_drain(cx))?; }
        let n = buf.len().min(MAX_RECORD_PLAIN);
        let rec = this.seal_record(&buf[..n])?;
        this.wbuf.drain(..this.wpos);
        this.wpos = 0;
        this.wbuf.extend_from_slice(&rec);
        // запись уже в буфере; недописанный хвост отправит следующий poll_write / poll_flush
        if let Poll::Ready(Err(e)) = this.poll_drain(cx) { return Poll::Ready(Err(e)); }
        Poll::Ready(Ok(n))
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        ready!(this.poll_drain(cx))?;
        Pin::new(&mut this.inner).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        ready!(this.poll_drain(cx))?;
        Pin::new(&mut this.inner).poll_shutdown(cx)
    }
}
