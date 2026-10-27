//! §10.1 / §10.3: TTL-очистка, номер версии записи, откат версии отвергается.

use p2p_overlay::dht::storage::{DhtRecord, DhtStore, RecordKey};
use p2p_overlay::identity::node_identity::NodeIdentity;

fn ident(tag: &str) -> NodeIdentity {
    let d = std::env::temp_dir().join(format!("p2p-rec-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    NodeIdentity::load_or_create(&d).unwrap()
}

fn rec(id: &NodeIdentity, v: &[u8], seq: u64) -> DhtRecord {
    DhtRecord::new_signed_seq(id, RecordKey::from_str("k"), v.to_vec(), 60, seq)
}

#[tokio::test]
async fn newer_sequence_replaces() {
    let id = ident("a1"); let s = DhtStore::default();
    assert!(s.store(rec(&id, b"v1", 10)).await);
    assert!(s.store(rec(&id, b"v2", 11)).await);
    assert_eq!(s.find(&RecordKey::from_str("k")).await.unwrap().value, b"v2");
}

#[tokio::test]
async fn sequence_rollback_rejected() {
    let id = ident("a2"); let s = DhtStore::default();
    assert!(s.store(rec(&id, b"new", 20)).await);
    assert!(!s.store(rec(&id, b"old", 19)).await, "откат версии должен отвергаться");
    assert_eq!(s.find(&RecordKey::from_str("k")).await.unwrap().value, b"new");
}

#[tokio::test]
async fn same_sequence_different_value_rejected_but_refresh_ok() {
    let id = ident("a3"); let s = DhtStore::default();
    assert!(s.store(rec(&id, b"x", 5)).await);
    assert!(!s.store(rec(&id, b"y", 5)).await, "тот же seq с другим значением — конфликт");
    assert!(s.store(rec(&id, b"x", 5)).await, "тот же seq и значение — refresh TTL");
}

#[test]
fn tampered_sequence_breaks_signature() {
    let id = ident("a4");
    let mut r = rec(&id, b"v", 7);
    assert!(r.verify());
    r.sequence = 8; // атакующий поднимает версию, чтобы вытеснить настоящую запись
    assert!(!r.verify(), "sequence входит в подпись");
}

#[tokio::test]
async fn expired_records_are_evicted() {
    let id = ident("a5"); let s = DhtStore::default();
    let mut r = rec(&id, b"v", 1);
    r.published_ms = r.published_ms.saturating_sub(120_000); // TTL 60 с давно истёк
    s.store(r).await;
    assert_eq!(s.len().await, 1);
    assert_eq!(s.evict_expired().await, 1);
    assert_eq!(s.len().await, 0);
}
