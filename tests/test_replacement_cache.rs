//! Кэш замены корзины (Kademlia §2.4) и кандидаты в ретрансляторы: при малом k узлы, не поместившиеся
//! в корзину или увиденные в ответах FIND_NODE, не теряются.

use p2p_overlay::dht::routing_table::{RoutingTable, REPLACEMENTS};
use p2p_overlay::types::{Contact, NodeId};

/// Узел из одной и той же (дальней) корзины относительно NodeID из нулей.
fn far(i: u8) -> Contact {
    let mut id = [0u8; 32];
    id[0] = 0x80;
    id[31] = i;
    Contact::new(NodeId(id), vec![i; 32], format!("127.0.0.1:{}", 7000 + i as u16).parse().unwrap())
}

#[test]
fn full_bucket_keeps_newcomer_in_replacement_cache() {
    let mut rt = RoutingTable::new(NodeId([0; 32]), 3);
    for i in 1..=3 { assert!(rt.update(far(i)).is_none()); }
    // корзина полна: возвращается старейший для PING, новый — в кэш замены, а не в никуда
    assert_eq!(rt.update(far(4)).map(|c| c.node_id), Some(far(1).node_id));
    assert_eq!(rt.total_contacts(), 3);
    assert_eq!(rt.replacement_count(), 1);
    let cands = rt.relay_candidates(&NodeId([0xff; 32]));
    assert_eq!(cands.len(), 4, "кандидаты в ретрансляторы — таблица + кэш");
}

#[test]
fn removed_contact_is_replaced_from_cache() {
    let mut rt = RoutingTable::new(NodeId([0; 32]), 3);
    for i in 1..=4 { rt.update(far(i)); }
    rt.remove(&far(2).node_id);
    assert_eq!(rt.total_contacts(), 3, "место выбывшего занял узел из кэша");
    assert!(rt.contains(&far(4).node_id));
    assert_eq!(rt.replacement_count(), 0);
}

#[test]
fn seen_contacts_go_to_cache_only_and_candidates_are_unique_and_sorted() {
    let mut rt = RoutingTable::new(NodeId([0; 32]), 3);
    rt.update(far(1));
    for i in 2..=6 { rt.note_seen(far(i)); }
    rt.note_seen(far(1));                    // уже в таблице — не дублируется
    rt.note_seen(Contact::new(NodeId([0; 32]), vec![0; 32], "127.0.0.1:1".parse().unwrap())); // себя — нет
    assert_eq!(rt.total_contacts(), 1, "непроверенные узлы в таблицу не попадают");
    assert_eq!(rt.replacement_count(), 5);
    let target = NodeId([0x80; 32]);
    let c = rt.relay_candidates(&target);
    assert_eq!(c.len(), 6);
    assert!(c.windows(2).all(|w| w[0].node_id.xor_distance(&target) <= w[1].node_id.xor_distance(&target)));
}

#[test]
fn replacement_cache_is_bounded() {
    let mut rt = RoutingTable::new(NodeId([0; 32]), 3);
    for i in 1..=40 { rt.note_seen(far(i)); }
    assert_eq!(rt.replacement_count(), REPLACEMENTS);
    // остаются самые свежие
    assert!(rt.relay_candidates(&NodeId([0; 32])).iter().any(|c| c.node_id == far(40).node_id));
}
