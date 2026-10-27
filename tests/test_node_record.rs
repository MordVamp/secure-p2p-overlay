//! §10.3 NodeRecord (подпись, TTL, откат, привязка к ключу) и §10.1 кворум STORE.

use std::sync::Arc;
use p2p_overlay::config::NodeConfig;
use p2p_overlay::dht::node::DhtNode;
use p2p_overlay::dht::storage::RecordKey;
use p2p_overlay::identity::node_identity::NodeIdentity;
use p2p_overlay::identity::record::{node_record_key, NodeRecord, RecordError};
use p2p_overlay::types::{now_unix, Contact};

fn ident(tag: &str) -> NodeIdentity {
    let d = std::env::temp_dir().join(format!("p2p-nr-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    NodeIdentity::load_or_create(&d).unwrap()
}

fn record(id: &NodeIdentity, seq: u64, ttl: u64) -> NodeRecord {
    NodeRecord::new(id.node_id, id.public_key_bytes(), vec!["127.0.0.1:7100".parse().unwrap()],
                    seq, ttl, |b| id.sign(b).to_bytes().to_vec())
}

#[test]
fn valid_record_roundtrips_through_dht_value() {
    let id = ident("v1");
    let r = record(&id, 1, 60);
    let back = NodeRecord::from_value(&r.to_value().unwrap()).unwrap().unwrap();
    assert!(back.validate_for_key(&node_record_key(&id.node_id)).is_ok());
    assert_eq!(back.addresses, r.addresses);
}

#[test]
fn non_node_record_value_is_not_parsed() {
    assert!(NodeRecord::from_value(b"hello").is_none());
}

#[test]
fn tampered_address_rejected() {
    let id = ident("t1");
    let mut r = record(&id, 1, 60);
    r.addresses = vec!["6.6.6.6:1".parse().unwrap()]; // подмена адреса
    assert!(matches!(r.validate(), Err(RecordError::InvalidSignature)));
}

#[test]
fn foreign_key_substitution_rejected() {
    let (a, b) = (ident("f1"), ident("f2"));
    let mut r = record(&a, 1, 60);
    r.identity_pubkey = b.public_key_bytes(); // чужой ключ под NodeID жертвы
    assert!(r.validate().is_err());
}

#[test]
fn record_under_wrong_dht_key_rejected() {
    let (a, b) = (ident("w1"), ident("w2"));
    let r = record(&a, 1, 60);
    assert!(matches!(r.validate_for_key(&node_record_key(&b.node_id)), Err(RecordError::WrongKey)));
}

#[test]
fn expired_record_rejected() {
    let id = ident("e1");
    let mut r = record(&id, 1, 60);
    // перепечатываем с expires_at в прошлом и валидной подписью
    r.issued_at = now_unix() - 120; r.expires_at = now_unix() - 60;
    r.signature = id.sign(&r.canonical_bytes()).to_bytes().to_vec();
    assert!(matches!(r.validate(), Err(RecordError::Expired)));
}

#[test]
fn future_issued_at_rejected() {
    let id = ident("e2");
    let mut r = record(&id, 1, 600);
    r.issued_at = now_unix() + 3600; r.expires_at = now_unix() + 7200;
    r.signature = id.sign(&r.canonical_bytes()).to_bytes().to_vec();
    assert!(matches!(r.validate(), Err(RecordError::FutureTimestamp)));
}

#[test]
fn sequence_rollback_rejected() {
    let id = ident("s1");
    let (old, new) = (record(&id, 5, 60), record(&id, 9, 60));
    assert!(NodeRecord::pick_newer(&new, &old).is_err(), "откат sequence_number");
    assert_eq!(NodeRecord::pick_newer(&old, &new).unwrap().sequence_number, 9);
}

fn dht_alone(quorum: usize, tag: &str) -> (DhtNode, NodeIdentity) {
    let id = ident(tag);
    let mut cfg = NodeConfig::default();
    cfg.dht.store_quorum = quorum;
    let me = Contact::new(id.node_id, id.public_key_bytes().to_vec(), "127.0.0.1:7199".parse().unwrap());
    (DhtNode::new(me, Arc::new(cfg)), id)
}

#[tokio::test]
async fn store_fails_without_quorum() {
    let (dht, id) = dht_alone(2, "q1"); // одинокий узел: подтверждена только локальная копия
    let r = dht.put(&id, RecordKey::from_str("k"), b"v".to_vec(), 60).await;
    assert!(r.is_err(), "1 реплика из 3 < кворума 2");
    assert!(r.unwrap_err().to_string().contains("нужно ≥2"));
}

#[tokio::test]
async fn store_succeeds_when_quorum_reached() {
    let (dht, id) = dht_alone(1, "q2");
    assert_eq!(dht.put(&id, RecordKey::from_str("k"), b"v".to_vec(), 60).await.unwrap(), 1);
}

#[tokio::test]
async fn resolve_own_node_record_locally() {
    let (dht, id) = dht_alone(1, "q3");
    dht.publish_node_record(&id).await.unwrap();
    let nr = dht.resolve_node(&id.node_id).await.unwrap();
    assert_eq!(nr.node_id, id.node_id);
}
