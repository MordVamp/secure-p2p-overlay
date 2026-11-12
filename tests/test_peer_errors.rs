//! Ошибки для других узлов (transport.peer_errors): в generic-режиме ретрансляторы не сообщают адресов и причин —
//! следующий узел назван NodeID; виновный в обрыве по-прежнему определяется.

use std::process::{Child, Command, Stdio};
use std::time::Duration;

use p2p_overlay::experiment::net::api;
use p2p_overlay::rpc::scrub_addresses;
use p2p_overlay::tunnel::profile::blamed_addr;

#[test]
fn addresses_scrubbed_but_node_ids_kept() {
    let s = scrub_addresses("next hop 10.0.0.5:22: TCP connect to [fe80::1]:7600: Connection refused; node:ab12cd34ef56ab78");
    assert!(!s.contains("10.0.0.5") && !s.contains("fe80"), "{s}");
    assert!(s.contains("node:ab12cd34ef56ab78"), "{s}");
    assert_eq!(scrub_addresses("seq=0, 3 hops, 1.5 МиБ"), "seq=0, 3 hops, 1.5 МиБ", "числа без порта — не адреса");
}

#[test]
fn blame_by_node_id_in_generic_chain() {
    let e = "first hop 127.0.0.1:7401 replied Error (seq=0): next hop node:00112233aabbccdd недоступен";
    assert_eq!(blamed_addr(e).as_deref(), Some("node:00112233aabbccdd"));
    let e = "first hop 127.0.0.1:7401 replied Error (seq=0): next hop node:00112233aabbccdd replied Error: ретранслятор node:99887766aabbccdd отказал";
    assert_eq!(blamed_addr(e).as_deref(), Some("node:00112233aabbccdd"), "отказал тот, кто ответил ошибкой");
}

struct Proc(Child);
impl Drop for Proc { fn drop(&mut self) { let _ = self.0.kill(); let _ = self.0.wait(); } }

fn node(base: u16, i: u16, boot: Option<u16>, mode: &str) -> Proc {
    let d = std::env::temp_dir().join(format!("p2p-perr-{}-{}", base + i, std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    let mut c = Command::new(env!("CARGO_BIN_EXE_p2p-node"));
    c.args(["-c", "config/node_config.yaml", "-p", &(base + i).to_string(), "--gui-port", &(base + 20 + i).to_string(),
            "--state-dir", &d.display().to_string(), "--metrics-dir", &d.join("m").display().to_string(),
            "--mailbox-poll", "0", "--peer-errors", mode]);
    if let Some(b) = boot { c.args(["-b", &format!("127.0.0.1:{b}")]); }
    Proc(c.stdout(Stdio::null()).stderr(Stdio::null()).spawn().expect("p2p-node"))
}

/// Сеть из 5 узлов; туннель 0 → 4; узел 4 убит; текст ошибки, дошедший до инициатора.
async fn chain_error(base: u16, mode: &str) -> (String, String) {
    let g = base + 20;
    let mut n = vec![Some(node(base, 0, None, mode))];
    for _ in 0..50 { if api(g, "GET", "/api/status", &[], 1).await.is_ok() { break; } tokio::time::sleep(Duration::from_millis(100)).await; }
    n.extend((1..5).map(|i| Some(node(base, i, Some(base), mode))));
    let mut ids = vec![];
    for i in 0..5u16 {
        let mut id = String::new();
        for _ in 0..100 {
            if let Ok(v) = api(g + i, "GET", "/api/status", &[], 1).await { id = v["node_id"].as_str().unwrap().into(); break; }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        ids.push(id);
    }
    tokio::time::sleep(Duration::from_secs(4)).await;
    let r = api(g, "POST", "/api/send", &[("target", &ids[4]), ("text", "до обрыва")], 30).await.unwrap();
    assert_eq!(r["delivered"], true, "{r}");
    n[4] = None; // получатель упал
    tokio::time::sleep(Duration::from_millis(300)).await;
    let _ = api(g, "POST", "/api/send", &[("target", &ids[4]), ("text", "после обрыва")], 60).await;
    // причина обрыва — в истории инициатора (само сообщение ушло в почтовый ящик DHT)
    let hist = api(g, "GET", "/api/history", &[("limit", "10")], 5).await.unwrap();
    let err = hist["entries"].as_array().unwrap().iter().rev().find_map(|e| e["error"].as_str().map(str::to_owned)).unwrap_or_default();
    (err, ids[4].clone())
}

#[tokio::test(flavor = "multi_thread")]
async fn generic_errors_name_nodes_not_addresses() {
    let (err, dead) = chain_error(8200, "generic").await;
    eprintln!("generic: {err}");
    // адрес первого узла цепочки инициатор знает сам; дальше — только NodeID
    let tail = err.split_once("replied").map(|x| x.1).unwrap_or("");
    assert!(!tail.contains("127.0.0.1"), "за первым узлом нет адресов: {err}");
    assert!(err.contains(&format!("node:{} недоступен", &dead[..16])), "упавший узел назван NodeID, без причины: {err}");
    assert!(!err.contains("refused") && !err.contains("os error"), "причина не раскрывается: {err}");
}

#[tokio::test(flavor = "multi_thread")]
async fn detailed_errors_keep_addresses() {
    let (err, _) = chain_error(8240, "detailed").await;
    eprintln!("detailed: {err}");
    let tail = err.split_once("replied").map(|x| x.1).unwrap_or("");
    assert!(tail.contains("127.0.0.1:"), "в отладочном режиме — адреса: {err}");
}
