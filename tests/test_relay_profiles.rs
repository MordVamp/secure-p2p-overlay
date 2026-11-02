//! §12 (продвинутый уровень): профили кандидатов, стратегии выбора ретрансляторов, определение виновного узла.

use std::collections::HashSet;
use p2p_overlay::tunnel::profile::{blamed_addr, subnet_of, RelayProfiles};
use p2p_overlay::types::{Contact, NodeId};

fn c(i: u8, host: &str) -> Contact { Contact::new(NodeId([i; 32]), vec![i; 32], format!("{host}:{}", 7000 + i as u16).parse().unwrap()) }

#[test]
fn blame_points_to_deepest_unreachable_node() {
    assert_eq!(blamed_addr("TCP connect to 127.0.0.1:7401: Connection refused").as_deref(), Some("127.0.0.1:7401"));
    // первый relay жив, второй недоступен
    let e = "first hop 127.0.0.1:7401 replied Error (seq=0): TCP connect to 127.0.0.1:7402: Connection refused";
    assert_eq!(blamed_addr(e).as_deref(), Some("127.0.0.1:7402"));
    // второй relay отказал сам (внесённый отказ)
    let e = "first hop 127.0.0.1:7401 replied Error (seq=0): next hop 127.0.0.1:7402 replied Error: ретранслятор 127.0.0.1:7402 отказал (внесённый отказ)";
    assert_eq!(blamed_addr(e).as_deref(), Some("127.0.0.1:7402"));
    // таймаут на первом hop
    assert_eq!(blamed_addr("first hop 127.0.0.1:7401 ACK timeout for seq=0").as_deref(), Some("127.0.0.1:7401"));
    // недоступен получатель — виноват его адрес (не relay)
    let e = "first hop 127.0.0.1:7401 replied Error (seq=0): next hop 127.0.0.1:7402 replied Error: TCP connect to 127.0.0.1:7409: refused";
    assert_eq!(blamed_addr(e).as_deref(), Some("127.0.0.1:7409"));
    assert_eq!(blamed_addr("tunnel not usable"), None);
}

#[test]
fn closest_keeps_xor_order_and_prefers_nodes_outside_pool() {
    let p = RelayProfiles::default();
    let cands = vec![c(1, "127.0.0.1"), c(2, "127.0.0.1"), c(3, "127.0.0.1"), c(4, "127.0.0.1")];
    let ids = |v: Vec<Contact>| v.iter().map(|x| x.node_id.0[0]).collect::<Vec<_>>();
    assert_eq!(ids(p.choose("closest", cands.clone(), 2, &HashSet::new())), vec![1, 2]);
    let avoid: HashSet<NodeId> = [NodeId([1; 32])].into();
    assert_eq!(ids(p.choose("closest", cands.clone(), 2, &avoid)), vec![2, 3], "узел другого туннеля пула — в последнюю очередь");
    let all: HashSet<NodeId> = cands.iter().map(|x| x.node_id).collect();
    assert_eq!(p.choose("closest", cands, 2, &all).len(), 2, "если иначе не набрать — допускаются и они");
}

#[test]
fn rating_avoids_failing_relays_and_tries_unknown_ones() {
    let mut p = RelayProfiles::default();
    let (good, bad, unknown) = (c(1, "10.0.0.1"), c(2, "10.0.1.1"), c(3, "10.0.2.1"));
    for _ in 0..5 { p.success(&good, 10.0); p.failure(&bad); }
    assert!(p.score(&good.node_id) > p.score(&unknown.node_id));
    assert!(p.score(&unknown.node_id) > p.score(&bad.node_id), "неизвестный лучше отказывающего");
    let pick = p.choose("rating", vec![bad.clone(), unknown.clone(), good.clone()], 2, &HashSet::new());
    assert_eq!(pick.iter().map(|x| x.node_id.0[0]).collect::<Vec<_>>(), vec![1, 3]);
    let prof = p.get(&bad.node_id).unwrap();
    assert_eq!((prof.failures, prof.consecutive_failures, prof.successes), (5, 5, 0));
    assert!(prof.ema_success < 0.1);
}

#[test]
fn slow_relay_ranks_below_fast_one() {
    let mut p = RelayProfiles::default();
    let (fast, slow) = (c(1, "10.0.0.1"), c(2, "10.0.1.1"));
    for _ in 0..5 { p.success(&fast, 5.0); p.success(&slow, 400.0); }
    assert!(p.score(&fast.node_id) > p.score(&slow.node_id));
}

#[test]
fn rating_prefers_distinct_subnets() {
    let p = RelayProfiles::default();
    let cands = vec![c(1, "10.0.0.1"), c(2, "10.0.0.2"), c(3, "10.0.5.1")];
    let pick = p.choose("rating", cands, 2, &HashSet::new());
    assert_eq!(pick.iter().map(|x| subnet_of(&x.host)).collect::<HashSet<_>>().len(), 2, "две разные /24");
}
