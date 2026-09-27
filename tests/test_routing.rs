//! tests/test_routing.rs — Тесты k-bucket T2.6–T2.9 (§24 ТЗ).

use p2p_overlay::dht::routing_table::{KBucket, RoutingTable};
use p2p_overlay::types::{Contact, NodeId};

fn make_contact(id_byte: u8, port: u16) -> Contact {
    Contact::new(
        NodeId([id_byte; 32]),
        vec![id_byte; 32],
        format!("127.0.0.1:{port}").parse().unwrap(),
    )
}

// T2.6: Вставка в неполный bucket — контакт добавляется в хвост (§18.2 п.3)
#[test]
fn t2_6_insert_to_non_full_bucket() {
    let mut bucket = KBucket::new(3);
    let c = make_contact(0xAA, 7001);
    let lru = bucket.update(c.clone());
    assert!(lru.is_none(), "No PING needed when bucket not full");
    assert_eq!(bucket.len(), 1);
}

// T2.7: Повторное наблюдение — контакт перемещается в хвост (§18.2 п.2)
#[test]
fn t2_7_known_contact_moves_to_tail() {
    let mut bucket = KBucket::new(3);
    let c1 = make_contact(0x01, 7001);
    let c2 = make_contact(0x02, 7002);
    let c3 = make_contact(0x03, 7003);

    bucket.update(c1.clone()); // [c1]
    bucket.update(c2.clone()); // [c1, c2]
    bucket.update(c3.clone()); // [c1, c2, c3]

    // c1 снова появляется — должен переместиться в хвост
    bucket.update(c1.clone()); // [c2, c3, c1]

    let contacts: Vec<_> = bucket.iter().collect();
    // Хвост (последний) = самый свежий = c1
    assert_eq!(contacts.last().unwrap().node_id, c1.node_id,
               "Re-seen contact must be at tail");
    // LRU (голова) = c2
    assert_eq!(contacts.first().unwrap().node_id, c2.node_id,
               "LRU must be at head after re-seen update");
}

// T2.8: Полный bucket + живой LRU → старый сохраняется, новый не добавляется (§18.2 п.5-6)
#[test]
fn t2_8_full_bucket_live_lru_preserved() {
    let mut bucket = KBucket::new(3);
    let c1 = make_contact(0x01, 7001);
    let c2 = make_contact(0x02, 7002);
    let c3 = make_contact(0x03, 7003);
    let c4 = make_contact(0x04, 7004); // новый кандидат

    bucket.update(c1.clone());
    bucket.update(c2.clone());
    bucket.update(c3.clone()); // bucket полон

    // Попытка добавить c4 — должна вернуть LRU для PING
    let lru = bucket.update(c4.clone());
    assert!(lru.is_some(), "Full bucket must return LRU for PING");
    assert_eq!(lru.unwrap().node_id, c1.node_id, "LRU should be c1 (first added)");

    // Имитируем: LRU ответил на PING → c4 не добавляется
    assert!(!bucket.contains(&c4.node_id), "c4 must NOT be added while LRU is alive");
    assert!(bucket.contains(&c1.node_id), "c1 (LRU) must stay in bucket");
}

// T2.9: Полный bucket + мёртвый LRU → LRU удаляется, новый добавляется (§18.2 п.5)
#[test]
fn t2_9_full_bucket_dead_lru_evicted() {
    let mut bucket = KBucket::new(3);
    let c1 = make_contact(0x01, 7001);
    let c2 = make_contact(0x02, 7002);
    let c3 = make_contact(0x03, 7003);
    let c4 = make_contact(0x04, 7004);

    bucket.update(c1.clone());
    bucket.update(c2.clone());
    bucket.update(c3.clone());

    let lru = bucket.update(c4.clone());
    assert!(lru.is_some());

    // Имитируем: LRU НЕ ответил → evict_lru_and_insert
    let lru_id = lru.unwrap().node_id;
    bucket.evict_lru_and_insert(c4.clone());

    assert!(!bucket.contains(&lru_id), "Dead LRU must be evicted");
    assert!(bucket.contains(&c4.node_id), "New contact must be added after eviction");
    assert_eq!(bucket.len(), 3, "Bucket size must remain at K=3");
}

// Дополнительно: RoutingTable find_closest сортирует по XOR
#[test]
fn t2_11_find_closest_sorted_by_xor() {
    let own = NodeId([0x00; 32]);
    let mut rt = RoutingTable::new(own, 3);

    let c_far   = Contact::new(NodeId([0xFFu8; 32]), vec![0u8; 32], "127.0.0.1:7001".parse().unwrap());
    let c_near  = Contact::new(NodeId([0x01u8; 32]), vec![0u8; 32], "127.0.0.1:7002".parse().unwrap());
    let c_mid   = Contact::new(NodeId([0x80u8; 32]), vec![0u8; 32], "127.0.0.1:7003".parse().unwrap());

    rt.update(c_far.clone());
    rt.update(c_near.clone());
    rt.update(c_mid.clone());

    let target  = NodeId([0x00; 32]);
    let closest = rt.find_closest(&target, 3);

    // Ближайший к [0;32] = [0x01;32] < [0x80;32] < [0xFF;32]
    assert_eq!(closest[0].node_id.0[0], 0x01, "First closest must be 0x01");
    assert_eq!(closest[1].node_id.0[0], 0x80, "Second must be 0x80");
    assert_eq!(closest[2].node_id.0[0], 0xFF, "Third must be 0xFF");
}
