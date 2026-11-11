//! Пул соединений между узлами (transport.reuse_connections, ТЗ §6 допускает оба подхода).
//!
//! TLS/AKE-контекст — один на процесс, поэтому узлы здесь — настоящие процессы `p2p-node`
//! (как на стенде экспериментов), а тест выступает только клиентом.

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::Duration;

use p2p_overlay::config::NodeConfig;
use p2p_overlay::experiment::net::api;
use p2p_overlay::identity::node_identity::NodeIdentity;
use p2p_overlay::rpc::{pool, PendingRpc, RpcClient};
use p2p_overlay::types::{Contact, NodeId};

fn dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("p2p-pool-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

struct Proc(Child);
impl Drop for Proc { fn drop(&mut self) { let _ = self.0.kill(); let _ = self.0.wait(); } }

fn node(state: &PathBuf, port: u16, gui: u16, boot: Option<u16>, extra: &[&str]) -> Proc {
    let mut c = Command::new(env!("CARGO_BIN_EXE_p2p-node"));
    c.args(["-c", "config/node_config.yaml", "-p", &port.to_string(), "--gui-port", &gui.to_string(),
            "--state-dir", &state.display().to_string(), "--metrics-dir", &state.join("m").display().to_string(),
            "--mailbox-poll", "0"]);
    if let Some(b) = boot { c.args(["-b", &format!("127.0.0.1:{b}")]); }
    std::fs::create_dir_all(state).unwrap();
    let log = std::fs::File::create(state.join("node.log")).unwrap();
    c.args(extra).stdout(Stdio::from(log.try_clone().unwrap())).stderr(Stdio::from(log));
    Proc(c.spawn().expect("p2p-node"))
}

async fn wait_api(gui: u16) -> serde_json::Value {
    for _ in 0..100 {
        if let Ok(v) = api(gui, "GET", "/api/status", &[], 2).await { return v; }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("узел :{gui} не поднялся");
}

/// Сеть из 5 узлов с пулом: сообщения через туннели доставляются, соединения переиспользуются.
#[tokio::test(flavor = "multi_thread")]
async fn messages_flow_over_pooled_connections() {
    let base = dir("net");
    // seed — первым и дожидаемся его: повторных попыток bootstrap у узла нет
    let mut _n = vec![node(&base.join("n0"), 7810, 7830, None, &["--reuse-connections"])];
    wait_api(7830).await;
    _n.extend((1..5u16).map(|i| node(&base.join(format!("n{i}")), 7810 + i, 7830 + i, Some(7810), &["--reuse-connections"])));
    let mut ids = vec![];
    for i in 0..5 { ids.push(wait_api(7830 + i).await["node_id"].as_str().unwrap().to_owned()); }
    tokio::time::sleep(Duration::from_secs(4)).await;   // bootstrap и self-lookup
    for k in 0..4 {
        let r = api(7830, "POST", "/api/send", &[("target", &ids[4]), ("text", &format!("pool {k}"))], 30).await.unwrap();
        assert_eq!(r["delivered"], true, "сообщение {k}: {r}");
    }
    let st = api(7830, "GET", "/api/status", &[], 5).await.unwrap();
    let p = &st["conn_pool"];
    assert_eq!(p["enabled"], true);
    assert!(p["reused"].as_u64().unwrap() > 0, "запросы шли по уже открытым соединениям: {p}");
    // без пула каждый запрос открывал бы соединение; с пулом соединений не больше, чем соседей
    assert!(p["open"].as_u64().unwrap() <= 4, "{p}");
    // область по умолчанию — tunnels: DHT (lookup к разным узлам) идёт соединением на каждый запрос и пул не трогает
    assert_eq!(p["scope"], "tunnels");
    let opened = p["opened"].as_u64().unwrap();
    for t in 1..5 {
        let r = api(7830, "POST", "/api/lookup", &[("target", &ids[t])], 20).await.unwrap();
        assert_eq!(r["ok"], true, "{r}");
    }
    let p2 = &api(7830, "GET", "/api/status", &[], 5).await.unwrap()["conn_pool"];
    assert_eq!(p2["opened"].as_u64().unwrap(), opened, "lookup не открывает соединений в пуле: {p2}");
}

/// Соединение из пула привязано к ожидаемому NodeID; после перезапуска соседа запрос проходит по новому.
#[tokio::test(flavor = "multi_thread")]
async fn pooled_connection_bound_to_node_id_and_survives_peer_restart() {
    // клиент в этом процессе: свой ключ, канал AKE, пул включён
    let me = NodeIdentity::load_or_create(&dir("client")).unwrap();
    p2p_overlay::rpc::install_ake(Arc::new(p2p_overlay::security::ake::AkeSecurity::from_identity(&me)));
    let mut cfg = NodeConfig::default();
    cfg.transport.reuse_connections = true;
    cfg.dht.rpc_timeout_ms = 800;   // для «молчащего» соседа в конце теста
    cfg.transport.pool_scope = "all".into();   // здесь пул проверяется на запросах DHT
    pool::install(&cfg.transport);
    let own = Contact::new(me.node_id, me.public_key_bytes().to_vec(), "127.0.0.1:7899".parse().unwrap());
    let rpc = RpcClient::new(Arc::new(PendingRpc::new()), &cfg);

    let state = dir("peer");
    let mut peer = node(&state, 7850, 7851, None, &["--channel", "ake"]);
    let id_hex = wait_api(7851).await["node_id"].as_str().unwrap().to_owned();
    let id = NodeId(hex::decode(&id_hex).unwrap().try_into().unwrap());
    let addr = "127.0.0.1:7850".parse().unwrap();
    let find = |exp: NodeId| { let (rpc, own) = (&rpc, own.clone()); async move {
        rpc.find_node(addr, 3000, 5000, &own, &exp, Some(&exp)).await } };

    find(id).await.expect("первый запрос");
    find(id).await.expect("второй — по тому же соединению");
    let (opened, reused, _) = pool::stats().await;
    assert_eq!(opened, 1, "одно соединение на два запроса");
    assert!(reused >= 1);

    // запрос «к другому узлу» по тому же адресу: открытое соединение не используется, рукопожатие отвергает
    let wrong = NodeId([7u8; 32]);
    assert!(find(wrong).await.is_err(), "NodeID в рукопожатии не совпал — ошибка, а не чужое соединение");

    // сосед перезапустился (тот же ключ и порт): старое соединение мертво, запрос повторяется по новому
    drop(peer);
    peer = node(&state, 7850, 7851, None, &["--channel", "ake"]);
    wait_api(7851).await;
    find(id).await.expect("после перезапуска соседа");
    assert_eq!(pool::stats().await.0, 2, "открыто новое соединение");
    drop(peer);

    // «полуоткрытое» соединение: сосед принял рукопожатие, но не отвечает (пропал без разрыва, NAT забыл).
    // После тайм-аута соединение закрывается — следующий запрос не должен ждать в том же мёртвом соединении
    let silent_id = NodeIdentity::load_or_create(&dir("silent")).unwrap();
    let silent = p2p_overlay::security::ake::AkeSecurity::from_identity(&silent_id);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:7852").await.unwrap();
    tokio::spawn(async move {
        let mut held = vec![];
        while let Ok((sock, _)) = listener.accept().await {
            if let Ok((s, _)) = silent.accept(sock).await { held.push(s); }   // держим и молчим
        }
    });
    let saddr = "127.0.0.1:7852".parse().unwrap();
    let opened = pool::stats().await.0;
    for _ in 0..2 {
        let r = rpc.find_node(saddr, 3000, 5000, &own, &silent_id.node_id, Some(&silent_id.node_id)).await;
        assert!(r.unwrap_err().to_string().contains("timeout"));
    }
    assert_eq!(pool::stats().await.0, opened + 2, "после тайм-аута — новое соединение, а не то же молчащее");
}
