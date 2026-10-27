//! §11.2: сквозное шифрование — relay не читает содержимое; подмена и чужой ключ отвергаются.

use p2p_overlay::identity::node_identity::NodeIdentity;
use p2p_overlay::security::e2e::{open, seal, Envelope};

fn ident(tag: &str) -> NodeIdentity {
    let d = std::env::temp_dir().join(format!("p2p-e2e-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    NodeIdentity::load_or_create(&d).unwrap()
}

const MSG: &[u8] = b"super secret message: meet at 10:00";

#[test]
fn roundtrip() {
    let bob = ident("b1");
    let env = seal(&bob.node_id, &bob.public_key_bytes(), MSG).unwrap();
    assert_eq!(open(&bob, &env).unwrap(), MSG);
}

#[test]
fn ciphertext_does_not_contain_plaintext() {
    let bob = ident("b2");
    let env = seal(&bob.node_id, &bob.public_key_bytes(), MSG).unwrap();
    assert!(!env.windows(MSG.len()).any(|w| w == MSG), "relay не должен видеть открытый текст");
    assert!(!env.windows(6).any(|w| w == b"secret"));
}

#[test]
fn relay_or_third_party_cannot_open() {
    let (bob, eve) = (ident("b3"), ident("e3"));
    let env = seal(&bob.node_id, &bob.public_key_bytes(), MSG).unwrap();
    assert!(open(&eve, &env).is_err(), "посторонний узел не расшифрует");
}

#[test]
fn eve_cannot_open_even_after_retargeting_envelope() {
    // Ева переписывает поле `to` на себя — AAD/ключ не сойдутся
    let (bob, eve) = (ident("b4"), ident("e4"));
    let raw = seal(&bob.node_id, &bob.public_key_bytes(), MSG).unwrap();
    let mut env: Envelope = rmp_serde::from_slice(&raw).unwrap();
    env.to = eve.node_id;
    let forged = rmp_serde::to_vec_named(&env).unwrap();
    assert!(open(&eve, &forged).is_err());
}

#[test]
fn tampered_ciphertext_rejected() {
    let bob = ident("b5");
    let raw = seal(&bob.node_id, &bob.public_key_bytes(), MSG).unwrap();
    let mut env: Envelope = rmp_serde::from_slice(&raw).unwrap();
    env.ct[0] ^= 1;
    assert!(open(&bob, &rmp_serde::to_vec_named(&env).unwrap()).is_err());
}

#[test]
fn tampered_ephemeral_key_or_nonce_rejected() {
    let bob = ident("b6");
    let raw = seal(&bob.node_id, &bob.public_key_bytes(), MSG).unwrap();
    let base: Envelope = rmp_serde::from_slice(&raw).unwrap();
    let mut a = base.clone(); a.eph[0] ^= 1;
    let mut b = base.clone(); b.nonce[0] ^= 1;
    assert!(open(&bob, &rmp_serde::to_vec_named(&a).unwrap()).is_err());
    assert!(open(&bob, &rmp_serde::to_vec_named(&b).unwrap()).is_err());
}

#[test]
fn seal_rejects_key_not_matching_node_id() {
    let (bob, eve) = (ident("b7"), ident("e7"));
    // подсовываем ключ Евы под NodeID Боба — отправитель не должен шифровать
    assert!(seal(&bob.node_id, &eve.public_key_bytes(), MSG).is_err());
}

#[test]
fn each_seal_is_unique() {
    let bob = ident("b8");
    let a = seal(&bob.node_id, &bob.public_key_bytes(), MSG).unwrap();
    let b = seal(&bob.node_id, &bob.public_key_bytes(), MSG).unwrap();
    assert_ne!(a, b, "эфемерный ключ и nonce свежие для каждого сообщения");
}

#[test]
fn plaintext_or_garbage_is_not_accepted() {
    let bob = ident("b9");
    assert!(open(&bob, MSG).is_err());
    assert!(open(&bob, &[]).is_err());
}
