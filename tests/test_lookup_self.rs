//! Итеративный lookup не возвращает и не опрашивает сам инициатора, даже если пиры его называют.
//! Иначе STORE засчитывал бы копию у самого автора как одну из R реплик (кворум §10.1).

use std::sync::Arc;
use tokio::{net::TcpListener, sync::Mutex};
use p2p_overlay::config::NodeConfig;
use p2p_overlay::dht::lookup::IterativeLookup;
use p2p_overlay::dht::routing_table::RoutingTable;
use p2p_overlay::identity::node_identity::NodeIdentity;
use p2p_overlay::rpc::{PendingRpc, RpcClient, RpcHandler};
use p2p_overlay::transport::connection::FramedStream;
use p2p_overlay::types::Contact;

fn ident(tag: &str) -> NodeIdentity {
    let d = std::env::temp_dir().join(format!("p2p-lkself-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    NodeIdentity::load_or_create(&d).unwrap()
}

#[tokio::test]
async fn lookup_excludes_initiator_even_if_peer_returns_it() {
    let cfg = Arc::new(NodeConfig::default());
    let (a, b) = (ident("a"), ident("b"));
    // B — настоящий сервер FIND_NODE; в его таблице есть A, поэтому B вернёт A в ответе
    let lb = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let b_addr = lb.local_addr().unwrap();
    // A «живёт» на закрытом порту: если lookup попытается опросить A, это будет видно как лишний RPC
    let a_contact = Contact::new(a.node_id, a.public_key_bytes().to_vec(), "127.0.0.1:1".parse().unwrap());
    let b_contact = Contact::new(b.node_id, b.public_key_bytes().to_vec(), b_addr);
    let b_rt = Arc::new(Mutex::new(RoutingTable::new(b.node_id, 3)));
    b_rt.lock().await.update(a_contact.clone());
    let hb = Arc::new(RpcHandler::new(b_contact.clone(), b_rt, Arc::new(PendingRpc::new()), cfg.clone()));
    tokio::spawn(async move {
        loop {
            let Ok((s, _)) = lb.accept().await else { break };
            let h = hb.clone();
            tokio::spawn(async move {
                let mut fs = FramedStream::new(s, 3000);
                while let Ok(f) = fs.recv().await {
                    if let Some(r) = h.handle(f).await { let _ = fs.send(&r).await; }
                }
            });
        }
    });

    let a_rt = Arc::new(Mutex::new(RoutingTable::new(a.node_id, 3)));
    a_rt.lock().await.update(b_contact);
    let pending = Arc::new(PendingRpc::new());
    let lk = IterativeLookup::new(a_contact.clone(), a_rt, Arc::new(RpcClient::new(pending, &cfg)), cfg.clone());
    // цель — сам A: ближе всех к ней A, т.е. без исключения он гарантированно попал бы в результат
    let r = lk.run(&a.node_id).await.unwrap();
    assert!(r.closest.iter().all(|c| c.node_id != a.node_id), "инициатор не должен быть в closest");
    assert_eq!(r.rpc_count, 1, "опрошен только B, не сам инициатор");
    assert!(r.queried_by_iter.iter().flatten().all(|q| *q != a.node_id.short()));
}
