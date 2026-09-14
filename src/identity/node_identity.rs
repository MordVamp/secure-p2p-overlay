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

pub struct NodeIdentity {
    pub node_id:    NodeId,
    pub public_key: VerifyingKey,
    signing_key:    SigningKey,
    pub state_dir:  PathBuf,
}

impl NodeIdentity {
    /// Загрузить или создать идентичность в `state_dir`.
    pub fn load_or_create(state_dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(state_dir)?;
        let key_path = state_dir.join("identity.key");

        let signing_key = if key_path.exists() {
            info!("Loading existing identity from {:?}", key_path);
            let pem = std::fs::read_to_string(&key_path)
                .context("reading identity.key")?;
            SigningKey::from_pkcs8_pem(&pem)
                .context("parsing identity.key")?
        } else {
            info!("Generating new identity, saving to {:?}", key_path);
            let key = SigningKey::generate(&mut OsRng);
            let pem = key.to_pkcs8_pem(LineEnding::LF)
                .context("encoding identity key")?;
            // Устанавливаем права 0600 до записи
            std::fs::write(&key_path, pem.as_bytes())
                .context("writing identity.key")?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o600))?;
            }
            key
        };

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
