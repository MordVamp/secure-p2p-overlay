//! identity/node_identity.rs — Долговременная идентичность узла.
//!
//! NodeID = SHA-256(canonical_encode(identity_public_key))
//! Закрытый ключ хранится в <state_dir>/identity.key (PEM).
//! Случайная генерация нового NodeID при каждом запуске запрещена.

use std::path::{Path, PathBuf};
use anyhow::{Context, Result};
use ed25519_dalek::{
    SigningKey, VerifyingKey, Signature, Signer, Verifier,
    pkcs8::{DecodePrivateKey, EncodePrivateKey},
};
use pkcs8::der::pem::LineEnding;
use rand::rngs::OsRng;
use sha2::{Sha256, Digest};
use tracing::info;

use crate::types::NodeId;

#[derive(Clone)]
pub struct NodeIdentity {
    pub node_id:    NodeId,
    pub public_key: VerifyingKey,
    signing_key:    SigningKey,
    pub state_dir:  PathBuf,
}

/// Записать файл с правами 0600 (права ставятся до записи содержимого).
fn write_private(path: &Path, data: &[u8]) -> Result<()> {
    let mut o = std::fs::OpenOptions::new();
    o.create(true).write(true).truncate(true);
    #[cfg(unix)]
    { use std::os::unix::fs::OpenOptionsExt; o.mode(0o600); }
    use std::io::Write;
    o.open(path)?.write_all(data)?;
    Ok(())
}

const ENC_AAD: &[u8] = b"p2p-overlay identity.key v1";

fn seal_key(kek: &[u8; 32], pem: &[u8]) -> Result<Vec<u8>> {
    use aes_gcm::{aead::{Aead, KeyInit, Payload}, Aes256Gcm, Nonce};
    use rand::RngCore;
    let mut nonce = [0u8; 12];
    OsRng.fill_bytes(&mut nonce);
    let ct = Aes256Gcm::new(kek.into()).encrypt(Nonce::from_slice(&nonce), Payload { msg: pem, aad: ENC_AAD })
        .map_err(|_| anyhow::anyhow!("шифрование identity.key"))?;
    Ok([nonce.as_slice(), &ct].concat())
}

fn open_key(kek: &[u8; 32], blob: &[u8]) -> Result<zeroize::Zeroizing<Vec<u8>>> {
    use aes_gcm::{aead::{Aead, KeyInit, Payload}, Aes256Gcm, Nonce};
    if blob.len() < 12 + 16 { anyhow::bail!("identity.key.enc повреждён"); }
    let pt = Aes256Gcm::new(kek.into()).decrypt(Nonce::from_slice(&blob[..12]), Payload { msg: &blob[12..], aad: ENC_AAD })
        .map_err(|_| anyhow::anyhow!("identity.key.enc: неверный ключ P2P_IDENTITY_KEK или файл изменён"))?;
    Ok(zeroize::Zeroizing::new(pt))
}

/// identity.key (PEM) или identity.key.enc (AES-256-GCM под KEK). С KEK открытый файл
/// шифруется и удаляется (миграция); без KEK зашифрованный файл не открыть — ошибка, а не новый ключ.
fn load_or_create_key(state_dir: &Path, kek: Option<&[u8; 32]>) -> Result<SigningKey> {
    let plain = state_dir.join("identity.key");
    let enc   = state_dir.join("identity.key.enc");
    if enc.exists() {
        let Some(kek) = kek else {
            anyhow::bail!("{} зашифрован: нужен P2P_IDENTITY_KEK", enc.display());
        };
        info!("Loading encrypted identity from {:?}", enc);
        let pem = open_key(kek, &std::fs::read(&enc).context("reading identity.key.enc")?)?;
        let pem = std::str::from_utf8(&pem).context("identity.key.enc: не PEM")?;
        return SigningKey::from_pkcs8_pem(pem).context("parsing identity.key.enc");
    }
    let key = if plain.exists() {
        info!("Loading existing identity from {:?}", plain);
        let pem = zeroize::Zeroizing::new(std::fs::read_to_string(&plain).context("reading identity.key")?);
        SigningKey::from_pkcs8_pem(&pem).context("parsing identity.key")?
    } else {
        info!("Generating new identity in {:?}", state_dir);
        SigningKey::generate(&mut OsRng)
    };
    let pem = key.to_pkcs8_pem(LineEnding::LF).context("encoding identity key")?;
    match kek {
        Some(kek) => {
            write_private(&enc, &seal_key(kek, pem.as_bytes())?).context("writing identity.key.enc")?;
            if plain.exists() {
                // затереть и удалить открытую копию
                let len = std::fs::metadata(&plain).map(|m| m.len() as usize).unwrap_or(0);
                let _ = write_private(&plain, &vec![0u8; len]);
                std::fs::remove_file(&plain).context("removing plaintext identity.key")?;
                info!("identity.key зашифрован → identity.key.enc");
            }
        }
        None if !plain.exists() => write_private(&plain, pem.as_bytes()).context("writing identity.key")?,
        None => {}
    }
    Ok(key)
}

impl NodeIdentity {
    /// Загрузить или создать идентичность в `state_dir`.
    pub fn load_or_create(state_dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(state_dir)?;
        // Ключ шифрования ключа (KEK, 32 байта hex) из окружения: на Android его хранит Keystore
        // и передаёт узлу при запуске. Тогда ключ лежит на диске только как identity.key.enc.
        let kek = std::env::var("P2P_IDENTITY_KEK").ok();
        std::env::remove_var("P2P_IDENTITY_KEK");
        let kek = match kek {
            Some(h) => {
                let b = zeroize::Zeroizing::new(hex::decode(h.trim()).context("P2P_IDENTITY_KEK: ожидается hex")?);
                let k: [u8; 32] = b.as_slice().try_into().map_err(|_| anyhow::anyhow!("P2P_IDENTITY_KEK: нужно 32 байта"))?;
                Some(zeroize::Zeroizing::new(k))
            }
            None => None,
        };
        Self::load_or_create_with_kek(state_dir, kek.as_deref())
    }

    /// То же с явным ключом шифрования ключа (без окружения): `Some` — ключ хранится только как
    /// `identity.key.enc` (открытый `identity.key` шифруется и удаляется), `None` — как раньше.
    pub fn load_or_create_with_kek(state_dir: &Path, kek: Option<&[u8; 32]>) -> Result<Self> {
        std::fs::create_dir_all(state_dir)?;
        let signing_key = load_or_create_key(state_dir, kek)?;

        let public_key = signing_key.verifying_key();
        let node_id    = compute_node_id(&public_key);

        info!("NodeID = {}", node_id);
        Ok(Self { node_id, public_key, signing_key, state_dir: state_dir.to_owned() })
    }

    /// Подписать произвольные данные identity-ключом.
    pub fn sign(&self, data: &[u8]) -> Signature {
        self.signing_key.sign(data)
    }

    /// Canonical-encoded открытый ключ (32 байта compressed point).
    /// Приватный ключ — только для построения TLS-сертификата узла.
    pub(crate) fn signing_key(&self) -> &SigningKey { &self.signing_key }

    pub fn public_key_bytes(&self) -> [u8; 32] {
        self.public_key.to_bytes()
    }
}

/// Вычислить NodeID из открытого ключа (детерминированно).
pub fn compute_node_id(pubkey: &VerifyingKey) -> NodeId {
    let mut hasher = Sha256::new();
    // Canonical encode: 1 байт префикс 0x01 (тип Ed25519) + 32 байта ключа
    hasher.update([0x01u8]);
    hasher.update(pubkey.to_bytes());
    let hash: [u8; 32] = hasher.finalize().into();
    NodeId(hash)
}

/// Проверить, что pubkey соответствует заявленному node_id.
pub fn verify_node_id(node_id: &NodeId, pubkey: &VerifyingKey) -> bool {
    compute_node_id(pubkey) == *node_id
}

/// Верифицировать подпись от конкретного узла.
pub fn verify_signature(pubkey: &VerifyingKey, data: &[u8], sig: &Signature) -> bool {
    pubkey.verify(data, sig).is_ok()
}

/// Вычислить NodeID из сырых байтов pubkey (для тестов и network payload).
pub fn compute_node_id_from_bytes(pubkey_bytes: &[u8]) -> anyhow::Result<NodeId> {
    let vk = VerifyingKey::from_bytes(pubkey_bytes.try_into().map_err(|_| anyhow::anyhow!("pubkey must be 32 bytes"))?)
        .map_err(|e| anyhow::anyhow!("invalid pubkey: {e}"))?;
    Ok(compute_node_id(&vk))
}

/// Проверить node_id по сырым байтам pubkey.
pub fn verify_node_id_bytes(node_id: &NodeId, pubkey_bytes: &[u8]) -> bool {
    compute_node_id_from_bytes(pubkey_bytes)
        .map(|id| id == *node_id)
        .unwrap_or(false)
}
