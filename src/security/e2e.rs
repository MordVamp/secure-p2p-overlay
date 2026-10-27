//! security/e2e.rs — сквозное (end-to-end) шифрование сообщений (ТЗ §11.2).
//!
//! Relay-узлы туннеля видят только `Envelope`; содержимое им недоступно.
//!
//! Схема (ECIES / sealed box, без интерактивного рукопожатия):
//!  1. Ключ получателя X25519 выводится из его Ed25519 identity-ключа
//!     (публичный — через Montgomery-форму, секретный — SHA-512(seed)[..32]).
//!  2. Отправитель генерирует эфемерную пару (e, E), считает `shared = X25519(e, R)`.
//!  3. Ключ `K = HKDF-SHA256(salt = E‖R, ikm = shared, info = "p2p-e2e-v1")`.
//!  4. `AES-256-GCM(K, nonce, plaintext, aad = "p2p-e2e-v1"‖to‖E)`.
//!
//! Аутентичность отправителя даёт подпись внутри `AppMessage` (sign-then-encrypt),
//! целостность и привязку к получателю — GCM + AAD.
//!
//! Ограничение: forward secrecy только по стороне отправителя; компрометация
//! долговременного ключа получателя раскрывает ранее перехваченные сообщения.

use aes_gcm::{aead::{Aead, Payload}, Aes256Gcm, KeyInit, Nonce};
use anyhow::{anyhow, bail, Result};
use hkdf::Hkdf;
use rand::{rngs::OsRng, RngCore};
use serde::{Deserialize, Serialize};
use sha2::{Sha256, Sha512, Digest};
use x25519_dalek::{EphemeralSecret, PublicKey, StaticSecret};

use crate::identity::node_identity::{verify_node_id_bytes, NodeIdentity};
use crate::types::NodeId;

const INFO: &[u8] = b"p2p-e2e-v1";
const VERSION: u8 = 1;

/// Зашифрованный контейнер — единственное, что видят relay.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Envelope {
    pub v:     u8,
    /// Адресат (открыто — нужен для маршрутизации и входит в AAD).
    pub to:    NodeId,
    /// Эфемерный публичный ключ отправителя (32 байта).
    pub eph:   Vec<u8>,
    pub nonce: Vec<u8>,
    pub ct:    Vec<u8>,
}

/// X25519-ключ из публичного Ed25519-ключа.
pub fn x25519_public_from_ed25519(ed_pub: &[u8]) -> Result<[u8; 32]> {
    let arr: [u8; 32] = ed_pub.try_into().map_err(|_| anyhow!("pubkey must be 32 bytes"))?;
    let vk = ed25519_dalek::VerifyingKey::from_bytes(&arr).map_err(|e| anyhow!("bad pubkey: {e}"))?;
    Ok(vk.to_montgomery().to_bytes())
}

fn x25519_secret(id: &NodeIdentity) -> StaticSecret {
    let h = Sha512::digest(id.signing_key().to_bytes());
    let mut s = [0u8; 32];
    s.copy_from_slice(&h[..32]);
    StaticSecret::from(s) // клэмпинг выполняет x25519
}

fn derive_key(shared: &[u8], eph: &[u8], recip: &[u8]) -> Result<[u8; 32]> {
    let mut salt = eph.to_vec();
    salt.extend_from_slice(recip);
    let mut key = [0u8; 32];
    Hkdf::<Sha256>::new(Some(&salt), shared)
        .expand(INFO, &mut key).map_err(|_| anyhow!("hkdf expand"))?;
    Ok(key)
}

fn aad(to: &NodeId, eph: &[u8]) -> Vec<u8> {
    let mut a = INFO.to_vec();
    a.extend_from_slice(&to.0);
    a.extend_from_slice(eph);
    a
}

/// Зашифровать `plaintext` для получателя. `recipient_ed_pub` обязан соответствовать `to`.
pub fn seal(to: &NodeId, recipient_ed_pub: &[u8], plaintext: &[u8]) -> Result<Vec<u8>> {
    if !verify_node_id_bytes(to, recipient_ed_pub) {
        bail!("E2E: ключ получателя не соответствует его NodeID");
    }
    let recip = x25519_public_from_ed25519(recipient_ed_pub)?;
    let e = EphemeralSecret::random_from_rng(OsRng);
    let eph = PublicKey::from(&e).to_bytes();
    let shared = e.diffie_hellman(&PublicKey::from(recip));
    if !shared.was_contributory() { bail!("E2E: слабый ключ получателя"); }
    let key = derive_key(shared.as_bytes(), &eph, &recip)?;

    let mut nonce = [0u8; 12];
    OsRng.fill_bytes(&mut nonce);
    let ct = Aes256Gcm::new_from_slice(&key).map_err(|_| anyhow!("aes key"))?
        .encrypt(Nonce::from_slice(&nonce), Payload { msg: plaintext, aad: &aad(to, &eph) })
        .map_err(|_| anyhow!("E2E: encrypt failed"))?;

    let env = Envelope { v: VERSION, to: *to, eph: eph.to_vec(), nonce: nonce.to_vec(), ct };
    rmp_serde::to_vec_named(&env).map_err(|e| anyhow!("encode envelope: {e}"))
}

/// Расшифровать контейнер своим identity. Любое несоответствие → Err.
pub fn open(me: &NodeIdentity, bytes: &[u8]) -> Result<Vec<u8>> {
    let env: Envelope = rmp_serde::from_slice(bytes).map_err(|e| anyhow!("E2E: не Envelope: {e}"))?;
    if env.v != VERSION { bail!("E2E: неподдерживаемая версия {}", env.v); }
    if env.to != me.node_id { bail!("E2E: сообщение адресовано не нам"); }
    let eph: [u8; 32] = env.eph.as_slice().try_into().map_err(|_| anyhow!("E2E: bad eph key"))?;
    if env.nonce.len() != 12 { bail!("E2E: bad nonce"); }

    let sk = x25519_secret(me);
    let my_pub = PublicKey::from(&sk).to_bytes();
    let shared = sk.diffie_hellman(&PublicKey::from(eph));
    if !shared.was_contributory() { bail!("E2E: слабый эфемерный ключ"); }
    let key = derive_key(shared.as_bytes(), &eph, &my_pub)?;

    Aes256Gcm::new_from_slice(&key).map_err(|_| anyhow!("aes key"))?
        .decrypt(Nonce::from_slice(&env.nonce), Payload { msg: &env.ct, aad: &aad(&env.to, &eph) })
        .map_err(|_| anyhow!("E2E: расшифровка не удалась (подмена или чужой ключ)"))
}
