//! Повтор bootstrap: узел, запущенный раньше seed, входит в сеть, когда seed поднимается.

use std::process::{Child, Command, Stdio};
use std::time::Duration;

use p2p_overlay::experiment::net::api;

struct Proc(Child);
impl Drop for Proc { fn drop(&mut self) { let _ = self.0.kill(); let _ = self.0.wait(); } }

fn node(tag: &str, port: u16, gui: u16, boot: Option<u16>) -> Proc {
    let d = std::env::temp_dir().join(format!("p2p-boot-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    let mut c = Command::new(env!("CARGO_BIN_EXE_p2p-node"));
    c.args(["-c", "config/node_config.yaml", "-p", &port.to_string(), "--gui-port", &gui.to_string(),
            "--state-dir", &d.display().to_string(), "--metrics-dir", &d.join("m").display().to_string(),
            "--mailbox-poll", "0"]);
    if let Some(b) = boot { c.args(["-b", &format!("127.0.0.1:{b}")]); }
    Proc(c.stdout(Stdio::null()).stderr(Stdio::null()).spawn().expect("p2p-node"))
}

async fn contacts(gui: u16) -> i64 {
    api(gui, "GET", "/api/status", &[], 2).await.ok()
        .and_then(|v| v["routing"]["total_contacts"].as_i64()).unwrap_or(-1)
}

#[tokio::test(flavor = "multi_thread")]
async fn node_started_before_seed_joins_when_seed_appears() {
    let _late = node("late", 7871, 7881, Some(7870));     // seed ещё не запущен
    tokio::time::sleep(Duration::from_millis(2500)).await;
    assert_eq!(contacts(7881).await, 0, "пока seed нет, таблица пуста");
    let _seed = node("seed", 7870, 7880, None);
    let mut joined = false;
    for _ in 0..80 {                                       // повторы 1, 2, 4 с — укладываемся в ~8 с
        if contacts(7881).await > 0 { joined = true; break; }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    assert!(joined, "узел вошёл в сеть после появления seed");
}
