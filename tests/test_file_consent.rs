//! Защита получателя файлов: согласие (files.accept = known), квоты места и одновременных приёмов,
//! пределы ждущих предложений; квоты DHT-хранилища и таблицы маршрутов ретранслятора.

use std::process::{Child, Command, Stdio};
use std::time::Duration;

use p2p_overlay::app::files::{FileOffer, FileStore, Quota, MAX_INCOMING_PER_PEER, MAX_PENDING_PER_PEER};
use p2p_overlay::experiment::net::api;
use p2p_overlay::types::NodeId;

fn offer(id: u8, size: u64) -> FileOffer {
    FileOffer { transfer_id: vec![id; 16], name: format!("f{id}.bin"), size, sha256: vec![0; 32],
                block_size: 40960, blocks: size.div_ceil(40960).max(1) as u32 }
}

fn store(tag: &str, quota: Quota) -> (FileStore, std::path::PathBuf) {
    let d = std::env::temp_dir().join(format!("p2p-consent-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    (FileStore::new(&d, 64 << 20, 60_000).with_quota(quota), d)
}

#[test]
fn quota_on_total_size_rejects_offer() {
    let (mut s, d) = store("quota", Quota { max_total_bytes: 3 << 20, min_free_bytes: 0 });
    let a = NodeId([1; 32]);
    s.on_offer(a, offer(1, 2 << 20), false).unwrap();
    let e = s.check_offer(NodeId([2; 32]), &offer(2, 2 << 20), false).unwrap_err().to_string();
    assert!(e.contains("квота"), "{e}");
    let _ = std::fs::remove_dir_all(d);
}

#[test]
fn concurrent_incoming_limited_per_peer() {
    let (mut s, d) = store("conc", Quota::default());
    let a = NodeId([1; 32]);
    for i in 0..MAX_INCOMING_PER_PEER as u8 { s.on_offer(a, offer(i + 1, 1000), false).unwrap(); }
    assert!(s.check_offer(a, &offer(50, 1000), false).is_err(), "третий приём от того же узла");
    assert!(s.check_offer(NodeId([2; 32]), &offer(51, 1000), false).is_ok(), "другой узел не задет");
    let _ = std::fs::remove_dir_all(d);
}

#[test]
fn pending_offers_limited_and_expire() {
    let (mut s, d) = store("pend", Quota::default());
    let a = NodeId([1; 32]);
    for i in 0..MAX_PENDING_PER_PEER as u8 { s.hold(a, offer(i + 1, 1000)).unwrap(); }
    assert!(s.hold(a, offer(60, 1000)).is_err(), "лишнее предложение отклоняется");
    std::thread::sleep(Duration::from_millis(20));
    assert_eq!(s.expire_pending(10).len(), MAX_PENDING_PER_PEER, "без ответа — отказ по тайм-ауту");
    let _ = std::fs::remove_dir_all(d);
}

// ── живые узлы: согласие на приём ─────────────────────────────────────────

struct Proc(Child);
impl Drop for Proc { fn drop(&mut self) { let _ = self.0.kill(); let _ = self.0.wait(); } }

fn node(i: u16, boot: Option<u16>) -> (Proc, std::path::PathBuf) {
    let d = std::env::temp_dir().join(format!("p2p-consent-n{i}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    let mut c = Command::new(env!("CARGO_BIN_EXE_p2p-node"));
    c.args(["-c", "config/node_config.yaml", "-p", &(8100 + i).to_string(), "--gui-port", &(8120 + i).to_string(),
            "--state-dir", &d.display().to_string(), "--metrics-dir", &d.join("m").display().to_string(),
            "--mailbox-poll", "0", "--file-accept", "known"]);
    if let Some(b) = boot { c.args(["-b", &format!("127.0.0.1:{b}")]); }
    (Proc(c.stdout(Stdio::null()).stderr(Stdio::null()).spawn().expect("p2p-node")), d)
}

async fn transfer(gui: u16, name: &str) -> serde_json::Value {
    for _ in 0..100 {
        let v = api(gui, "GET", "/api/files", &[], 5).await.unwrap();
        if let Some(t) = v["transfers"].as_array().and_then(|a| a.iter().find(|t| t["name"] == name && t["dir"] == "in").cloned()) {
            return t;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("на :{gui} нет передачи {name}");
}

#[tokio::test(flavor = "multi_thread")]
async fn stranger_file_waits_for_consent_known_peer_is_automatic() {
    let mut n = vec![node(0, None)];
    for _ in 0..50 { if api(8120, "GET", "/api/status", &[], 1).await.is_ok() { break; } tokio::time::sleep(Duration::from_millis(100)).await; }
    n.extend((1..5).map(|i| node(i, Some(8100))));
    let mut ids = vec![];
    for i in 0..5u16 {
        let mut id = String::new();
        for _ in 0..100 {
            if let Ok(v) = api(8120 + i, "GET", "/api/status", &[], 1).await { id = v["node_id"].as_str().unwrap().into(); break; }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        ids.push(id);
    }
    tokio::time::sleep(Duration::from_secs(4)).await;
    let src = n[0].1.join("src");
    std::fs::create_dir_all(&src).unwrap();
    for (f, len) in [("a.bin", 300_000usize), ("b.bin", 200_000), ("c.bin", 100_000)] {
        std::fs::write(src.join(f), vec![7u8; len]).unwrap();
    }
    let path = |f: &str| src.join(f).display().to_string();

    // незнакомец: файл ждёт решения; отказ получателя доходит до отправителя
    let (pa, t4) = (path("a.bin"), ids[4].clone());
    let send = tokio::spawn(async move { api(8120, "POST", "/api/send_file", &[("target", &t4), ("path", &pa)], 120).await.unwrap() });
    let t = transfer(8124, "a.bin").await;
    assert_eq!(t["status"], "offered", "{t}");
    assert!(!n[4].1.join("downloads").join(&ids[0][..16]).join("a.bin").exists(), "до согласия на диск ничего не пишется");
    assert!(t["warning"].as_str().unwrap_or("").contains("не из ваших контактов"), "причина в предложении: {t}");
    let tid = t["transfer_id"].as_str().unwrap().to_owned();
    assert_eq!(api(8124, "POST", "/api/file/reject", &[("transfer_id", &tid)], 10).await.unwrap()["ok"], true);
    let r = send.await.unwrap();
    assert_eq!(r["ok"], false, "{r}");
    assert!(r["error"].as_str().unwrap_or("").contains("отклонил"), "{r}");

    // незнакомец: пользователь принял — файл доставлен
    let (pb, t4) = (path("b.bin"), ids[4].clone());
    let send = tokio::spawn(async move { api(8120, "POST", "/api/send_file", &[("target", &t4), ("path", &pb)], 120).await.unwrap() });
    let t = transfer(8124, "b.bin").await;
    assert_eq!(t["status"], "offered");
    api(8124, "POST", "/api/file/accept", &[("transfer_id", t["transfer_id"].as_str().unwrap())], 10).await.unwrap();
    let r = send.await.unwrap();
    assert_eq!(r["ok"], true, "{r}");
    assert_eq!(std::fs::read(n[4].1.join("downloads").join(&ids[0][..16]).join("b.bin")).unwrap().len(), 200_000, "в папке отправителя");

    // знакомый (получатель сам писал отправителю) — маленький файл принимается сам
    let w = api(8124, "POST", "/api/send", &[("target", &ids[0]), ("text", "привет")], 30).await.unwrap();
    assert_eq!(w["delivered"], true, "{w}");
    let r = api(8120, "POST", "/api/send_file", &[("target", &ids[4]), ("path", &path("c.bin"))], 60).await.unwrap();
    assert_eq!(r["ok"], true, "знакомому — без вопроса: {r}");

    // но исполняемый файл со скрытыми символами в имени — всегда с вопросом и предупреждением
    let spoof = src.join("счёт_\u{202e}fdp.exe");
    std::fs::write(&spoof, b"MZ fake").unwrap();
    let (ps, t4) = (spoof.display().to_string(), ids[4].clone());
    let send = tokio::spawn(async move { api(8120, "POST", "/api/send_file", &[("target", &t4), ("path", &ps)], 120).await.unwrap() });
    let t = transfer(8124, "счёт_\u{202e}fdp.exe").await;
    assert_eq!(t["status"], "offered", "даже от знакомого: {t}");
    assert_eq!(t["ext"], "exe");
    assert!(t["warning"].as_str().unwrap_or("").contains("U+202E"), "{t}");
    api(8124, "POST", "/api/file/reject", &[("transfer_id", t["transfer_id"].as_str().unwrap())], 10).await.unwrap();
    let _ = send.await;
    drop(n);
}

// ── квоты DHT-хранилища и таблицы маршрутов ретранслятора ─────────────────

#[tokio::test]
async fn dht_store_quota_per_publisher() {
    use p2p_overlay::dht::storage::{DhtRecord, DhtStore, RecordKey};
    fn rec(k: &str, p: u8, len: usize) -> DhtRecord { DhtRecord::new(RecordKey::from_str(k), vec![0; len], NodeId([p; 32]), 60) }

    // ADR-49: доля публикатора — не отказ, а вытеснение его же давно не нужных записей
    let s = DhtStore::new(1024);
    let (_, per_pub) = s.publisher_quota();
    assert_eq!(per_pub, 256);
    for i in 0..per_pub { assert!(s.store(rec(&format!("box-{i}"), 1, 10)).await); }
    assert!(s.find(&RecordKey::from_str("box-0")).await.is_some(), "box-0 читают — она нужна");
    assert!(s.store(rec("box-new", 1, 10)).await, "новая запись принята, а не отвергнута");
    assert!(s.find(&RecordKey::from_str("box-0")).await.is_some(), "нужная запись осталась");
    assert!(s.find(&RecordKey::from_str("box-1")).await.is_none(), "вытеснена самая давно не нужная");
    assert_eq!(s.usage().await.1, per_pub, "у публикатора по-прежнему не больше его доли");

    // квота по объёму: 1/16 общего объёма
    let s = DhtStore::with_limits(1024, 1 << 20);
    let (bytes, _) = s.publisher_quota();
    assert_eq!(bytes, 64 * 1024);
    for i in 0..20 { assert!(s.store(rec(&format!("big-{i}"), 1, 10_000)).await); }
    assert!(s.usage().await.0 <= bytes, "крупные записи одного публикатора не больше 1/16 объёма");
    assert!(!s.store(rec("huge", 1, 100_000)).await, "запись больше доли публикатора отвергается");
    assert!(s.store(rec("other", 2, 10)).await, "другой публикатор по-прежнему пишет");

    // общее переполнение: несколько «личностей» злоумышленника вытесняют свои записи, а не записи честного узла,
    // пока у честного не больше справедливой доли (при неограниченном числе личностей доля стремится к нулю — Sybil, T34)
    let s = DhtStore::with_limits(48, 32 << 20);
    for i in 0..5 { assert!(s.store(rec(&format!("honest-{i}"), 9, 100)).await); }
    for p in 1..=4u8 { for i in 0..16 { assert!(s.store(rec(&format!("spam-{p}-{i}"), p, 100)).await); } }
    assert_eq!(s.usage().await.1, 48, "хранилище заполнено, но не переполнено");
    for i in 0..5 {
        assert!(s.find(&RecordKey::from_str(&format!("honest-{i}"))).await.is_some(), "честная запись honest-{i} на месте");
    }
}

#[tokio::test]
async fn relay_routes_quota_per_initiator() {
    use p2p_overlay::tunnel::routes::{TunnelRoutes, MAX_ROUTES_PER_INITIATOR};
    let r = TunnelRoutes::new();
    let addr: std::net::SocketAddr = "127.0.0.1:9".parse().unwrap();
    let a = NodeId([1; 32]);
    let mut ok = 0;
    for i in 0..(MAX_ROUTES_PER_INITIATOR as u32 + 10) {
        let mut tid = [0u8; 16];
        tid[..4].copy_from_slice(&i.to_be_bytes());
        if r.insert_for(a, tid, addr, None, 60).await { ok += 1; }
    }
    assert_eq!(ok, MAX_ROUTES_PER_INITIATOR);
    assert!(r.insert_for(NodeId([2; 32]), [0xff; 16], addr, None, 60).await, "другой инициатор не задет");
}

#[test]
fn quota_applies_only_to_automatic_acceptance() {
    let (mut s, d) = store("byuser", Quota { max_total_bytes: 3 << 20, min_free_bytes: 0 });
    s.on_offer(NodeId([1; 32]), offer(1, 2 << 20), false).unwrap();
    assert!(s.check_offer(NodeId([2; 32]), &offer(2, 2 << 20), false).is_err(), "сам — не больше квоты");
    assert!(s.check_offer(NodeId([2; 32]), &offer(2, 2 << 20), true).is_ok(), "принятый пользователем — только свободное место");
    let _ = std::fs::remove_dir_all(d);
}

#[test]
fn human_sizes_and_peer_folder() {
    use p2p_overlay::app::files::{human_bytes, peer_dir};
    assert_eq!(human_bytes(3 * 1024 * 1024 * 1024 + 200 * 1024 * 1024), "3,2 ГиБ");
    assert_eq!(human_bytes(20 * 1024 * 1024), "20,0 МиБ");
    assert_eq!(peer_dir(&NodeId([0xab; 32])), "abababababababab");
}

#[test]
fn very_large_files_go_to_separate_subfolder() {
    use p2p_overlay::app::files::{LARGE_DIR, LARGE_FILE_BYTES};
    let (s, d) = store("large", Quota::default());
    let a = NodeId([0xcd; 32]);
    assert_eq!(s.target_dir(&a, LARGE_FILE_BYTES), d.join("downloads/cdcdcdcdcdcdcdcd"), "ровно 2 ГиБ — обычная папка");
    assert_eq!(s.target_dir(&a, LARGE_FILE_BYTES + 1), d.join("downloads/cdcdcdcdcdcdcdcd").join(LARGE_DIR));
}

fn node_at(base: u16, i: u16, boot: Option<u16>) -> (Proc, std::path::PathBuf) {
    let d = std::env::temp_dir().join(format!("p2p-queue-n{}-{}", base + i, std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    let mut c = Command::new(env!("CARGO_BIN_EXE_p2p-node"));
    c.args(["-c", "config/node_config.yaml", "-p", &(base + i).to_string(), "--gui-port", &(base + 20 + i).to_string(),
            "--state-dir", &d.display().to_string(), "--metrics-dir", &d.join("m").display().to_string(),
            "--mailbox-poll", "0", "--file-accept", "known"]);
    if let Some(b) = boot { c.args(["-b", &format!("127.0.0.1:{b}")]); }
    (Proc(c.stdout(Stdio::null()).stderr(Stdio::null()).spawn().expect("p2p-node")), d)
}

#[tokio::test(flavor = "multi_thread")]
async fn sender_queue_settings_quota_warning_and_delete() {
    const B: u16 = 8160;
    const G: u16 = B + 20;
    let mut n = vec![node_at(B, 0, None)];
    for _ in 0..50 { if api(G, "GET", "/api/status", &[], 1).await.is_ok() { break; } tokio::time::sleep(Duration::from_millis(100)).await; }
    n.extend((1..5).map(|i| node_at(B, i, Some(B))));
    let mut ids = vec![];
    for i in 0..5u16 {
        let mut id = String::new();
        for _ in 0..100 {
            if let Ok(v) = api(G + i, "GET", "/api/status", &[], 1).await { id = v["node_id"].as_str().unwrap().into(); break; }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        ids.push(id);
    }
    tokio::time::sleep(Duration::from_secs(4)).await;
    // получатель (4) «знает» отправителя (0), квота — 16 МиБ (минимум)
    let w = api(G + 4, "POST", "/api/send", &[("target", &ids[0]), ("text", "привет")], 30).await.unwrap();
    assert_eq!(w["delivered"], true, "{w}");
    let s = api(G + 4, "POST", "/api/settings/files", &[("quota_mb", "16")], 10).await.unwrap();
    assert_eq!(s["quota_mb"], 16, "{s}");
    assert!(api(G + 4, "POST", "/api/settings/files", &[("accept", "всегда")], 10).await.unwrap()["ok"] == false);

    // 4 фото сразу: получатель принимает по 2, остальные ждут в очереди отправителя — доходят все
    let src = n[0].1.join("src");
    std::fs::create_dir_all(&src).unwrap();
    let mut sends = vec![];
    for k in 0..4 {
        let p = src.join(format!("photo{k}.jpg"));
        std::fs::write(&p, vec![k as u8; 3_900_000]).unwrap();   // ~3,7 МиБ — меньше порога 16 МиБ, принимается сам
        let (ps, t) = (p.display().to_string(), ids[4].clone());
        sends.push(tokio::spawn(async move { api(G, "POST", "/api/send_file", &[("target", &t), ("path", &ps)], 600).await.unwrap() }));
    }
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let f = api(G, "GET", "/api/files", &[], 5).await.unwrap();
    let queued = f["transfers"].as_array().unwrap().iter().filter(|t| t["status"] == "queued").count();
    assert!(queued >= 1, "часть файлов ждёт в очереди: {f}");
    for s in sends { let r = s.await.unwrap(); assert_eq!(r["ok"], true, "{r}"); }

    // 4 × 3,7 МиБ ≈ 14,9 из 16 МиБ — больше 90 %: предупреждение
    let s = api(G + 4, "GET", "/api/settings/files", &[], 5).await.unwrap();
    assert_eq!(s["quota_warn"], true, "{s}");
    // квота заполнена: файл от знакомого — не отказ, а вопрос с объяснением
    let p5 = src.join("photo5.jpg");
    std::fs::write(&p5, vec![5u8; 3_900_000]).unwrap();
    let (ps, t) = (p5.display().to_string(), ids[4].clone());
    let send5 = tokio::spawn(async move { api(G, "POST", "/api/send_file", &[("target", &t), ("path", &ps)], 120).await.unwrap() });
    let t5 = transfer(G + 4, "photo5.jpg").await;
    assert_eq!(t5["status"], "offered", "{t5}");
    assert!(t5["warning"].as_str().unwrap_or("").contains("заполнено"), "{t5}");
    api(G + 4, "POST", "/api/file/reject", &[("transfer_id", t5["transfer_id"].as_str().unwrap())], 10).await.unwrap();
    let _ = send5.await;
    // удалить один файл — освобождается место, предупреждение уходит
    let d = api(G + 4, "GET", "/api/downloads", &[], 5).await.unwrap();
    let path = d["folders"][0]["files"][0]["path"].as_str().unwrap().to_owned();
    let r = api(G + 4, "POST", "/api/downloads/delete", &[("path", &path)], 5).await.unwrap();
    assert_eq!(r["ok"], true, "{r}");
    assert!(api(G + 4, "POST", "/api/downloads/delete", &[("path", "/etc/passwd")], 5).await.unwrap()["ok"] == false,
            "вне downloads/ — нельзя");
    let s = api(G + 4, "GET", "/api/settings/files", &[], 5).await.unwrap();
    assert_eq!(s["quota_warn"], false, "{s}");
    drop(n);
}
