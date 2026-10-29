//! node_proc.rs — запуск и остановка локального `p2p-node` из приложения.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use crate::settings::Settings;

pub struct LocalNode {
    child:        Child,
    pub log_path: PathBuf,
    pub bin:      PathBuf,
}

/// Найти `p2p-node`: явный путь → рядом с приложением → target/{debug,release} выше по дереву → PATH.
pub fn find_node_binary(explicit: &str) -> PathBuf {
    if !explicit.trim().is_empty() { return PathBuf::from(explicit.trim()); }
    let name = if cfg!(windows) { "p2p-node.exe" } else { "p2p-node" };
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let p = dir.join(name);
            if p.exists() { return p; }
            for anc in dir.ancestors() {
                for prof in ["debug", "release"] {
                    let p = anc.join("target").join(prof).join(name);
                    if p.exists() { return p; }
                }
            }
        }
    }
    PathBuf::from(name)
}

/// Корень репозитория (есть config/node_config.yaml) — чтобы узел взял общий конфиг.
fn repo_root(bin: &Path) -> Option<PathBuf> {
    bin.ancestors().find(|a| a.join("config/node_config.yaml").exists()).map(Path::to_path_buf)
}

impl LocalNode {
    pub fn start(s: &Settings) -> Result<Self, String> {
        let bin = find_node_binary(&s.node_bin);
        let state = PathBuf::from(&s.state_dir);
        std::fs::create_dir_all(&state).map_err(|e| format!("{}: {e}", state.display()))?;
        let log_path = state.join("node.log");
        let log = std::fs::File::create(&log_path).map_err(|e| format!("{}: {e}", log_path.display()))?;
        let mut cmd = Command::new(&bin);
        cmd.arg("-p").arg(s.listen_port.to_string())
           .arg("--gui-port").arg(s.gui_port.to_string())
           .arg("--state-dir").arg(&state)
           .arg("--metrics-dir").arg(state.join("metrics"));
        if let Some(root) = repo_root(&bin) {
            cmd.arg("-c").arg(root.join("config/node_config.yaml")).current_dir(root);
        }
        for b in s.bootstrap.split([',', ' ', ';']).map(str::trim).filter(|b| !b.is_empty()) {
            cmd.arg("-b").arg(b);
        }
        let err = log.try_clone().map_err(|e| e.to_string())?;
        let child = cmd.stdin(Stdio::null()).stdout(Stdio::from(log)).stderr(Stdio::from(err))
            .spawn().map_err(|e| format!("не удалось запустить {}: {e}", bin.display()))?;
        Ok(Self { child, log_path, bin })
    }

    pub fn running(&mut self) -> bool { matches!(self.child.try_wait(), Ok(None)) }

    pub fn pid(&self) -> u32 { self.child.id() }

    /// Корректная остановка (SIGTERM → узел сбрасывает метрики), через 3 с — kill.
    pub fn stop(&mut self) {
        if !self.running() { return; }
        #[cfg(unix)]
        { let _ = Command::new("kill").arg("-TERM").arg(self.child.id().to_string()).status(); }
        let t0 = Instant::now();
        while t0.elapsed() < Duration::from_secs(3) {
            if !self.running() { return; }
            std::thread::sleep(Duration::from_millis(50));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }

    /// Последние `n` строк журнала узла (без ANSI-последовательностей).
    pub fn log_tail(&self, n: usize) -> Vec<String> {
        let Ok(bytes) = std::fs::read(&self.log_path) else { return vec![] };
        let start = bytes.len().saturating_sub(64 * 1024);
        let text = strip_ansi(&String::from_utf8_lossy(&bytes[start..]));
        let lines: Vec<&str> = text.lines().collect();
        lines[lines.len().saturating_sub(n)..].iter().map(|l| l.to_string()).collect()
    }
}

impl Drop for LocalNode {
    fn drop(&mut self) { self.stop(); }
}

fn strip_ansi(s: &str) -> String {
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
