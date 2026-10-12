//! tests/test_identity.rs — Тесты идентичности T2.1–T2.5 (§24 ТЗ).

use std::{fs, path::PathBuf};
use p2p_overlay::identity::node_identity::{
    NodeIdentity, compute_node_id_from_bytes, verify_node_id_bytes
};
use p2p_overlay::types::NodeId;

// T2.1: Перезапуск с тем же state_dir → NodeID не меняется
#[test]
fn t2_1_identity_stable_across_restarts() {
    let dir = tempdir("t21a");
    let id1 = NodeIdentity::load_or_create(&dir).unwrap();
    let id2 = NodeIdentity::load_or_create(&dir).unwrap();
    assert_eq!(id1.node_id, id2.node_id, "NodeID must be stable across restarts");
}

// T2.2: Два новых узла с разными state_dir → разные NodeID
#[test]
fn t2_2_different_nodes_different_ids() {
    let dir1 = tempdir("t22a");
    let dir2 = tempdir("t22b");
    let id1 = NodeIdentity::load_or_create(&dir1).unwrap();
    let id2 = NodeIdentity::load_or_create(&dir2).unwrap();
    assert_ne!(id1.node_id, id2.node_id, "Different nodes must have different NodeIDs");
}

// T2.3: NodeID детерминированно вычисляется из pubkey-байт
#[test]
fn t2_3_node_id_from_pubkey_deterministic() {
    let dir = tempdir("t23");
    let identity = NodeIdentity::load_or_create(&dir).unwrap();
    let pk_bytes = identity.public_key_bytes();
    let id1 = compute_node_id_from_bytes(&pk_bytes).unwrap();
    let id2 = compute_node_id_from_bytes(&pk_bytes).unwrap();
    assert_eq!(id1, id2, "NodeID computation must be deterministic");
    assert_eq!(id1, identity.node_id, "Computed NodeID must match stored");
}

// T2.4: Контакт с несоответствующим node_id и pubkey отклоняется
#[test]
fn t2_4_mismatched_pubkey_rejected() {
    let dir1 = tempdir("t24a");
    let dir2 = tempdir("t24b");
    let id1 = NodeIdentity::load_or_create(&dir1).unwrap();
    let id2 = NodeIdentity::load_or_create(&dir2).unwrap();

    // NodeID от id1 не должен совпадать с pubkey id2
    let ok = verify_node_id_bytes(&id1.node_id, &id2.public_key_bytes());
    assert!(!ok, "NodeID from id1 must not match pubkey of id2");
}

// T2.5: XOR-расстояние и сортировка совпадают с эталонными значениями
#[test]
fn t2_5_xor_metric_reference_vectors() {
    let a = NodeId([0x00u8; 32]);
    let b = NodeId([0xFFu8; 32]);
    let c = NodeId({ let mut x = [0u8; 32]; x[0] = 0x80; x });

    assert_eq!(a.xor_distance(&b), [0xFFu8; 32], "XOR(a,b) must be max");
    assert_eq!(a.xor_distance(&a), [0u8; 32],    "XOR(a,a) must be 0");
    assert_eq!(a.xor_distance(&c)[0], 0x80,       "XOR(a,c) first byte = 0x80");

    // Сортировка: a(0), c(0x80…), b(0xFF…) — ближайшие к a
    let target = a;
    let mut dists = vec![b.xor_distance(&target), c.xor_distance(&target), a.xor_distance(&target)];
    dists.sort();
    assert_eq!(dists[0], [0u8; 32]);       // a
    assert_eq!(dists[1][0], 0x80);          // c
    assert_eq!(dists[2], [0xFFu8; 32]);    // b
}

// ── helpers ──────────────────────────────────────────────────────────────────

fn tempdir(tag: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!("p2p_test_{}_{}", tag,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH).unwrap().subsec_nanos()));
    fs::create_dir_all(&p).unwrap();
    p
}

// ── E21: NodeID сохраняется при перезапуске ───────────────────────────────────


fn temp_state_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("p2p_test_{name}_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

// E21: load_or_create() с одним и тем же state_dir дважды даёт одинаковый NodeID.
#[test]
fn e21_node_id_stable_across_restarts() {
    use p2p_overlay::identity::node_identity::NodeIdentity;

    let dir = temp_state_dir("e21_stable");

    let id1 = NodeIdentity::load_or_create(&dir).unwrap();
    let node_id_first = id1.node_id;
    let pubkey_first  = id1.public_key_bytes();
    drop(id1);

    // "Перезапуск" — создаём новый NodeIdentity с тем же путём
    let id2 = NodeIdentity::load_or_create(&dir).unwrap();
    assert_eq!(node_id_first, id2.node_id,
        "E21: NodeID must be identical after restart");
    assert_eq!(pubkey_first, id2.public_key_bytes(),
        "E21: public key must be identical after restart");

    // Очистка
    std::fs::remove_dir_all(&dir).ok();
}

// E21b: Разные state_dir → разные NodeID.
#[test]
fn e21_different_dirs_different_ids() {
    use p2p_overlay::identity::node_identity::NodeIdentity;

    let dir_a = temp_state_dir("e21_diff_a");
    let dir_b = temp_state_dir("e21_diff_b");

    let id_a = NodeIdentity::load_or_create(&dir_a).unwrap();
    let id_b = NodeIdentity::load_or_create(&dir_b).unwrap();

    assert_ne!(id_a.node_id, id_b.node_id,
        "E21: different state dirs must produce different NodeIDs");

    std::fs::remove_dir_all(&dir_a).ok();
    std::fs::remove_dir_all(&dir_b).ok();
}

// ── E22: Проверка NodeID/pubkey ───────────────────────────────────────────────

// E22: verify_node_id_bytes корректна для валидного pubkey.
#[test]
fn e22_verify_node_id_valid_pubkey() {
    use p2p_overlay::identity::node_identity::{NodeIdentity, verify_node_id_bytes};

    let dir = temp_state_dir("e22_valid");
    let id  = NodeIdentity::load_or_create(&dir).unwrap();

    assert!(
        verify_node_id_bytes(&id.node_id, &id.public_key_bytes()),
        "E22: NodeID computed from pubkey must verify correctly"
    );
    std::fs::remove_dir_all(&dir).ok();
}

// E22: verify_node_id_bytes отклоняет чужой pubkey при заявленном NodeID.
#[test]
fn e22_verify_node_id_wrong_pubkey_rejected() {
    use p2p_overlay::identity::node_identity::{NodeIdentity, verify_node_id_bytes};

    let dir_a = temp_state_dir("e22_rej_a");
    let dir_b = temp_state_dir("e22_rej_b");

    let id_a = NodeIdentity::load_or_create(&dir_a).unwrap();
    let id_b = NodeIdentity::load_or_create(&dir_b).unwrap();

    // Проверяем NodeID узла A с pubkey узла B — должно вернуть false
    assert!(
        !verify_node_id_bytes(&id_a.node_id, &id_b.public_key_bytes()),
        "E22: mismatched pubkey must NOT verify against NodeID"
    );

    std::fs::remove_dir_all(&dir_a).ok();
    std::fs::remove_dir_all(&dir_b).ok();
}
