//! experiment/net.rs — набор живых процессов `p2p-node` для одного прогона сценария
//! и клиент их HTTP API. Оркестратор знает все NodeID, узлам передаётся только адрес
//! bootstrap-узла (ТЗ §15.1).

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU16, Ordering};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::process::{Child, Command};

const PORT_FIRST: u16 = 7400;
const PORT_LAST: u16 = 9000;
static NEXT_PORT: AtomicU16 = AtomicU16::new(PORT_FIRST);

/// Блок портов на сеть из `n` узлов: `n` портов узлов, затем `n` портов API.
fn alloc_ports(n: usize) -> u16 {
    let need = (2 * n + 4) as u16;
    let base = NEXT_PORT.fetch_add(need, Ordering::Relaxed);
    if base + need > PORT_LAST {
        NEXT_PORT.store(PORT_FIRST + need, Ordering::Relaxed);
        return PORT_FIRST;
    }
    base
}

fn enc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Запрос к HTTP API узла на `127.0.0.1:gui`. Не-200 — ошибка с текстом узла.
pub async fn api(gui: u16, method: &str, path: &str, query: &[(&str, &str)], timeout_s: u64) -> Result<Value> {
    let mut target = path.to_owned();
    if !query.is_empty() {
        target.push('?');
        target.push_str(&query.iter().map(|(k, v)| format!("{}={}", enc(k), enc(v))).collect::<Vec<_>>().join("&"));
    }
    let io = async {
        let mut s = TcpStream::connect(("127.0.0.1", gui)).await?;
        let req = format!("{method} {target} HTTP/1.1\r\nHost: 127.0.0.1:{gui}\r\nConnection: close\r\nContent-Length: 0\r\n\r\n");
        s.write_all(req.as_bytes()).await?;
        let mut buf = Vec::new();
        s.read_to_end(&mut buf).await?;
        anyhow::Ok(buf)
    };
    let buf = tokio::time::timeout(Duration::from_secs(timeout_s), io).await
        .with_context(|| format!("API :{gui}{path}: тайм-аут {timeout_s} с"))??;
    let text = String::from_utf8_lossy(&buf);
    let (head, body) = text.split_once("\r\n\r\n").context("некорректный HTTP-ответ")?;
    let v: Value = serde_json::from_str(body).with_context(|| format!("не JSON от :{gui}: {body}"))?;
    let status: u16 = head.split_whitespace().nth(1).and_then(|x| x.parse().ok()).unwrap_or(0);
    if status != 200 {
        bail!("HTTP {status} от :{gui}{path}: {}", v["error"].as_str().unwrap_or(""));
    }
    Ok(v)
}

fn spawn_child(bin: &Path, args: &[String], log: &Path) -> Result<Child> {
    let f = std::fs::OpenOptions::new().create(true).append(true).open(log)
        .with_context(|| format!("журнал {}", log.display()))?;
    let e = f.try_clone()?;
    Command::new(bin).args(args)
        .stdin(Stdio::null()).stdout(Stdio::from(f)).stderr(Stdio::from(e))
        .kill_on_drop(true)
        .spawn().with_context(|| format!("запуск {}", bin.display()))
}

pub struct Node {
    pub host: String,
    pub port: u16,
    pub gui:  u16,
    pub id:   String,
    args:     Vec<String>,
    child:    Option<Child>,
    log:      PathBuf,
}

/// Bootstrap-адреса узла `i` (индексы узлов) по схеме (ТЗ §14):
/// star — все к узлу 0; ring — к узлу i−1; tree — двоичное дерево, к родителю (i−1)/2;
/// multiseed — три seed (0, 1, 2) знают друг друга, остальные — все три seed.
pub fn bootstrap_of(topology: &str, i: usize, n: usize) -> Vec<usize> {
    if i == 0 { return vec![]; }
    match topology {
        "ring" => vec![i - 1],
        "tree" => vec![(i - 1) / 2],
        "multiseed" => (0..3.min(n)).filter(|&s| s != i && (i >= 3 || s < i)).collect(),
        _ => vec![0],
    }
}

/// Узлы одного прогона; схема bootstrap — см. `bootstrap_of`.
pub struct Net {
    /// Каталог логов, ключей и метрик узлов: `logs/scenarios/<tag>_<unix>_<порт>` (вне git).
    pub dir:      PathBuf,
    pub nodes:    Vec<Node>,
    pub t_spawn:  Instant,
    /// Время запуска всех процессов (узлы стартуют с шагом 0.15 с).
    pub launch_span_s: f64,
    bin:          PathBuf,
}

impl Net {
    pub async fn start(tag: &str, n: usize, topology: &str) -> Result<Self> {
        Self::start_with(tag, n, topology, |_| vec![]).await
    }

    /// То же с дополнительными аргументами для каждого узла (канал, внесение отказов и т.п.).
    pub async fn start_with(tag: &str, n: usize, topology: &str, extra: impl Fn(usize) -> Vec<String>) -> Result<Self> {
        Self::start_hosts(tag, n, topology, |_| "127.0.0.1".to_owned(), extra).await
    }

    /// То же, узел `i` — на адресе `host(i)` (разные loopback-адреса = разные «подсети» на одной машине).
    pub async fn start_hosts(tag: &str, n: usize, topology: &str, host: impl Fn(usize) -> String,
                             extra: impl Fn(usize) -> Vec<String>) -> Result<Self> {
        let bin = crate::gui::orch_gui::node_binary();
        if !bin.exists() { bail!("нет {} — выполните `cargo build --bins`", bin.display()); }
        let base = alloc_ports(n);
        let unix = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        let dir = PathBuf::from("logs/scenarios").join(format!("{tag}_{unix}_{base}"));
        std::fs::create_dir_all(&dir)?;
        let mut net = Self { dir, nodes: vec![], t_spawn: Instant::now(), launch_span_s: 0.0, bin };
        for i in 0..n {
            let port = base + i as u16;
            let gui = port + n as u16 + 2;
            let mut args = vec![
                "-p".into(), port.to_string(), "--gui-port".into(), gui.to_string(),
                "--state-dir".into(), net.dir.join(format!("state{i}")).display().to_string(),
                "--metrics-dir".into(), net.dir.join(format!("metrics{i}")).display().to_string(),
            ];
            let h = host(i);
            if h != "127.0.0.1" { args.push("--host".into()); args.push(h.clone()); }
            let var = super::variant();
            if var.config != super::report::CONFIG { args.push("-c".into()); args.push(var.config.clone()); }
            let ex = extra(i);
            if let Some(c) = &var.channel {
                if !ex.iter().any(|a| a == "--channel") { args.push("--channel".into()); args.push(c.clone()); }
            }
            args.extend(ex);
            for b in bootstrap_of(topology, i, n) {
                args.push("-b".into());
                args.push(format!("{}:{}", host(b), base + b as u16));
            }
            let log = net.dir.join(format!("n{i}.log"));
            let child = spawn_child(&net.bin, &args, &log)?;
            net.nodes.push(Node { host: h, port, gui, id: String::new(), args, child: Some(child), log });
            tokio::time::sleep(Duration::from_millis(if i == 0 { 500 } else { 150 })).await;
        }
        net.launch_span_s = net.t_spawn.elapsed().as_secs_f64();
        for i in 0..n { net.nodes[i].id = net.wait_id(i).await?; }
        Ok(net)
    }

    async fn wait_id(&self, i: usize) -> Result<String> {
        for _ in 0..100 {
            if let Ok(st) = self.status(i).await {
                if let Some(id) = st["node_id"].as_str() { return Ok(id.to_owned()); }
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        bail!("API узла {i} (:{}) не поднялся — см. {}", self.nodes[i].gui, self.nodes[i].log.display())
    }

    pub fn n(&self) -> usize { self.nodes.len() }

    pub fn id(&self, i: usize) -> String { self.nodes[i].id.clone() }

    pub fn addr(&self, i: usize) -> String { format!("{}:{}", self.nodes[i].host, self.nodes[i].port) }

    pub async fn api(&self, i: usize, method: &str, path: &str, query: &[(&str, &str)]) -> Result<Value> {
        api(self.nodes[i].gui, method, path, query, 40).await
    }

    pub async fn status(&self, i: usize) -> Result<Value> {
        api(self.nodes[i].gui, "GET", "/api/status", &[], 10).await
    }

    /// Число контактов в таблице узла; −1, если узел не отвечает.
    pub async fn contacts(&self, i: usize) -> i64 {
        match self.status(i).await {
            Ok(st) => st["routing"]["total_contacts"].as_i64().unwrap_or(-1),
            Err(_) => -1,
        }
    }

    pub async fn contact_ids(&self, i: usize) -> Result<HashSet<String>> {
        let st = self.status(i).await?;
        Ok(st["routing"]["buckets"].as_array().into_iter().flatten()
            .flat_map(|b| b["contacts"].as_array().cloned().unwrap_or_default())
            .filter_map(|c| c["node_id"].as_str().map(str::to_owned))
            .collect())
    }

    pub fn is_alive(&mut self, i: usize) -> bool {
        match &mut self.nodes[i].child {
            Some(c) => matches!(c.try_wait(), Ok(None)),
            None => false,
        }
    }

    pub fn alive(&mut self) -> Vec<usize> { (0..self.n()).filter(|&i| self.is_alive(i)).collect() }

    /// Отказ узла: SIGKILL без корректного завершения.
    pub async fn kill(&mut self, i: usize) {
        if let Some(mut c) = self.nodes[i].child.take() {
            let _ = c.start_kill();
            let _ = c.wait().await;
        }
    }

    /// Перезапуск с той же identity и портом; bootstrap — любой живой узел.
    pub async fn restart(&mut self, i: usize) -> Result<()> {
        if self.is_alive(i) { self.kill(i).await; }
        let peer = self.alive().into_iter().find(|&j| j != i).context("нет живых узлов для bootstrap")?;
        let mut args = self.nodes[i].args.clone();
        while let Some(k) = args.iter().position(|a| a == "-b") { args.drain(k..k + 2); }
        args.push("-b".into());
        args.push(self.addr(peer));
        let child = spawn_child(&self.bin, &args, &self.nodes[i].log)?;
        self.nodes[i].args = args;
        self.nodes[i].child = Some(child);
        self.wait_id(i).await?;
        Ok(())
    }

    /// Подготовка (не метрика): ждём, пока сумма контактов живых узлов не меняется
    /// `stable` опросов подряд (раз в 1 с). Возвращает время ожидания, с (NaN — тайм-аут).
    pub async fn converge(&mut self, timeout_s: f64, stable: usize) -> f64 {
        let t0 = Instant::now();
        let (mut last, mut same) = (None, 0);
        while t0.elapsed().as_secs_f64() < timeout_s {
            let mut sum = 0;
            for i in self.alive() { sum += self.contacts(i).await.max(0); }
            same = if last == Some(sum) { same + 1 } else { 0 };
            last = Some(sum);
            if same >= stable { return t0.elapsed().as_secs_f64(); }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        f64::NAN
    }

    /// Корректная остановка всех узлов (SIGTERM → узел сбрасывает метрики), через 5 с — SIGKILL.
    pub async fn stop(&mut self) {
        for n in &self.nodes {
            if let Some(pid) = n.child.as_ref().and_then(|c| c.id()) {
                let _ = std::process::Command::new("kill").args(["-TERM", &pid.to_string()]).status();
            }
        }
        for n in &mut self.nodes {
            if let Some(mut c) = n.child.take() {
                if tokio::time::timeout(Duration::from_secs(5), c.wait()).await.is_err() {
                    let _ = c.start_kill();
                    let _ = c.wait().await;
                }
            }
        }
    }

    /// Скопировать журналы узлов в `dst` (без ANSI-цветов) — логи контрольного прогона в результатах.
    pub fn save_logs(&self, dst: &Path) -> Result<()> {
        std::fs::create_dir_all(dst)?;
        for (i, n) in self.nodes.iter().enumerate() {
            let text = std::fs::read(&n.log).map(|b| strip_ansi(&String::from_utf8_lossy(&b))).unwrap_or_default();
            std::fs::write(dst.join(format!("n{i}.log")), text)?;
        }
        Ok(())
    }
}

pub fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c == '\u{1b}' && it.peek() == Some(&'[') {
            it.next();
            for d in it.by_ref() { if d.is_ascii_alphabetic() { break; } }
        } else {
            out.push(c);
        }
    }
    out
}
