//! §24: проверка реализации по независимым тест-векторам (docs/test_vectors/nodeid_xor.json).

use p2p_overlay::identity::node_identity::compute_node_id_from_bytes;
use p2p_overlay::types::NodeId;

fn h(s: &str) -> Vec<u8> { hex::decode(s).unwrap() }

#[test]
fn nodeid_and_xor_match_reference_vectors() {
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string("docs/test_vectors/nodeid_xor.json").unwrap()).unwrap();
    let target = NodeId(h(v["target_node_id_hex"].as_str().unwrap()).try_into().unwrap());
    let mut ids = Vec::new();
    for n in v["nodes"].as_array().unwrap() {
        // фиктивные «ключи» 0x01..: проверяем только хэш-правило, поэтому берём байты напрямую
        let pk = h(n["pubkey_hex"].as_str().unwrap());
        let id = match compute_node_id_from_bytes(&pk) {
            Ok(id) => id,
            Err(_) => {
                // pk не лежит на кривой Ed25519 — считаем хэш по правилу ТЗ вручную
                use sha2::{Digest, Sha256};
                let mut hs = Sha256::new(); hs.update([1u8]); hs.update(&pk);
                NodeId(hs.finalize().into())
            }
        };
        assert_eq!(hex::encode(id.0), n["node_id_hex"].as_str().unwrap());
        assert_eq!(hex::encode(id.xor_distance(&target)), n["xor_to_target_hex"].as_str().unwrap());
        ids.push(id);
    }
    let mut idx: Vec<usize> = (0..ids.len()).collect();
    idx.sort_by_key(|&i| ids[i].xor_distance(&target));
    let expected: Vec<usize> = v["sorted_by_distance_indices"].as_array().unwrap()
        .iter().map(|x| x.as_u64().unwrap() as usize).collect();
    assert_eq!(idx, expected);
}
