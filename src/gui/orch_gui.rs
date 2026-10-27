//! gui/orch_gui.rs — веб-панель оркестратора: запуск/остановка нескольких
//! узлов (дочерние процессы `p2p-node`), у каждого своя панель на gui_port.

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::json;
use tokio::process::{Child, Command};
use tokio::sync::Mutex;

use super::http::{handler, serve, Request, Response};

const PAGE: &str = include_str!("orch.html");
const MAX_NODES: usize = 20;
const NODE_BASE_PORT: u16 = 7100;
pub const GUI_OFFSET: u16 = 1000;

struct Spawned { port: u16, child: Child }

pub struct Orchestrator {
    nodes:    Mutex<Vec<Spawned>>,
    config:   PathBuf,
    /// Каталог state/metrics/logs запускаемых узлов.
    work_dir: PathBuf,
}

fn node_binary() -> PathBuf {
    let exe = std::env::current_exe().unwrap_or_default();
    exe.with_file_name(if cfg!(windows) { "p2p-node.exe" } else { "p2p-node" })
}

impl Orchestrator {
    pub fn new(config: PathBuf) -> Arc<Self> { Self::new_in(config, PathBuf::from(".")) }

    pub fn new_in(config: PathBuf, work_dir: PathBuf) -> Arc<Self> {
        Arc::new(Self { nodes: Mutex::new(Vec::new()), config, work_dir })
    }

    pub async fn ports(&self) -> Vec<u16> { self.nodes.lock().await.iter().map(|n| n.port).collect() }

    pub async fn spawn(&self, count: usize) -> Result<Vec<u16>, String> {
        let bin = node_binary();
        if !bin.exists() {
            return Err(format!("нет {} — выполните `cargo build --bins`", bin.display()));
        }
        let mut nodes = self.nodes.lock().await;
        if nodes.len() + count > MAX_NODES { return Err(format!("максимум {MAX_NODES} узлов")); }
        let mut started = Vec::new();
        for _ in 0..count {
            let port = (NODE_BASE_PORT..NODE_BASE_PORT + 200)
                .find(|p| !nodes.iter().any(|n| n.port == *p))
                .ok_or("нет свободных портов")?;
            // bootstrap = самый первый узел (seed), кроме самого первого
            let w = &self.work_dir;
            std::fs::create_dir_all(w.join("logs")).map_err(|e| format!("mkdir logs: {e}"))?;
            let log = std::fs::File::create(w.join("logs").join(format!("node-{port}.log")))
                .map_err(|e| format!("log file: {e}"))?;
            let log_err = log.try_clone().map_err(|e| format!("log file: {e}"))?;
            let mut cmd = Command::new(&bin);
            cmd.arg("--config").arg(&self.config)
               .arg("--port").arg(port.to_string())
               .arg("--gui-port").arg((port + GUI_OFFSET).to_string())
               .arg("--state-dir").arg(w.join(format!("state/orch-{port}")))
               .arg("--metrics-dir").arg(w.join(format!("metrics/orch-{port}")))
               .env("RUST_LOG", "p2p_overlay=info")
               .kill_on_drop(true)
               .stdout(std::process::Stdio::from(log))
               .stderr(std::process::Stdio::from(log_err));
            if let Some(seed) = nodes.first() {
                cmd.arg("--bootstrap").arg(format!("127.0.0.1:{}", seed.port));
            }
            let child = cmd.spawn().map_err(|e| format!("spawn: {e}"))?;
            nodes.push(Spawned { port, child });
            started.push(port);
        }
        Ok(started)
    }

    pub async fn kill(&self, port: u16) -> bool {
        let mut nodes = self.nodes.lock().await;
        if let Some(i) = nodes.iter().position(|n| n.port == port) {
            let mut n = nodes.remove(i);
            let _ = n.child.kill().await;
            true
        } else { false }
    }

    async fn list(&self) -> serde_json::Value {
        let mut nodes = self.nodes.lock().await;
        let list: Vec<_> = nodes.iter_mut().map(|n| json!({
            "port": n.port,
            "gui_port": n.port + GUI_OFFSET,
            "pid": n.child.id(),
            "alive": matches!(n.child.try_wait(), Ok(None)),
            "seed": false,
        })).collect();
        json!({ "nodes": list })
    }
}

pub async fn run(orch: Arc<Orchestrator>, port: u16) -> anyhow::Result<()> {
    serve(port, handler(move |req| route(orch.clone(), req))).await
}

async fn route(o: Arc<Orchestrator>, req: Request) -> Response {
    let post = req.method == "POST";
    match (req.path.as_str(), post) {
        ("/", false) => Response::html(PAGE),
        ("/api/nodes", false) => Response::json(o.list().await),
        ("/api/spawn", true) => {
            let n = req.query.get("count").and_then(|c| c.parse::<usize>().ok()).unwrap_or(1).clamp(1, 10);
            match o.spawn(n).await {
                Ok(p)  => Response::json(json!({ "ok": true, "ports": p })),
                Err(e) => Response::json(json!({ "ok": false, "error": e })),
            }
        }
        ("/api/kill", true) => {
            let Some(p) = req.query.get("port").and_then(|c| c.parse::<u16>().ok())
                else { return Response::error(400, "port"); };
            Response::json(json!({ "ok": o.kill(p).await }))
        }
        ("/api/killall", true) => {
            let ports: Vec<u16> = o.nodes.lock().await.iter().map(|n| n.port).collect();
            for p in ports { o.kill(p).await; }
            Response::json(json!({ "ok": true }))
        }
        _ => Response::error(404, "not found"),
    }
}
