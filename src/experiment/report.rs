//! experiment/report.rs — каталог результатов прогона, `meta.json`, CSV и статистика (ТЗ §15.3).

use std::path::PathBuf;

use anyhow::{Context, Result};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::Args;

pub const CONFIG: &str = "config/node_config.yaml";

/// n / min / max / медиана / среднее; p95 — только при n ≥ 20 (ТЗ §15.3).
pub struct Stat { pub n: usize, pub min: f64, pub max: f64, pub median: f64, pub mean: f64, pub p95: Option<f64> }

pub fn stat(xs: &[f64]) -> Stat {
    let mut v: Vec<f64> = xs.iter().copied().filter(|x| x.is_finite()).collect();
    v.sort_by(|a, b| a.total_cmp(b));
    let n = v.len();
    if n == 0 { return Stat { n, min: f64::NAN, max: f64::NAN, median: f64::NAN, mean: f64::NAN, p95: None }; }
    let median = if n % 2 == 1 { v[n / 2] } else { (v[n / 2 - 1] + v[n / 2]) / 2.0 };
    let p95 = (n >= 20).then(|| v[((0.95 * n as f64).ceil() as usize).min(n) - 1]);
    Stat { n, min: v[0], max: v[n - 1], median, mean: v.iter().sum::<f64>() / n as f64, p95 }
}

pub fn fmt(xs: &[f64]) -> String {
    let s = stat(xs);
    if s.n == 0 { return "n=0".into(); }
    let mut out = format!("n={}, min {:.1}, медиана {:.1}, среднее {:.1}, max {:.1}", s.n, s.min, s.median, s.mean, s.max);
    if let Some(p) = s.p95 { out += &format!(", p95 {p:.1}"); }
    out
}

pub fn f1(x: f64) -> String { if x.is_finite() { format!("{x:.1}") } else { String::new() } }

/// Значение JSON → ячейка CSV (null → пусто).
pub fn jv(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

pub fn git(args: &[&str]) -> String {
    std::process::Command::new("git").args(args).output().ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
        .unwrap_or_default()
}

/// Точная версия кода при незакоммиченных изменениях: sha256 diff и новых файлов (кроме результатов).
fn worktree_hash() -> Option<String> {
    if git(&["status", "--porcelain"]).is_empty() { return None; }
    let mut h = Sha256::new();
    if let Ok(o) = std::process::Command::new("git").args(["diff", "HEAD", "--binary"]).output() { h.update(&o.stdout); }
    let mut untracked: Vec<String> = git(&["ls-files", "--others", "--exclude-standard"]).lines()
        .filter(|f| !f.starts_with("experiments/results/")).map(str::to_owned).collect();
    untracked.sort();
    for f in untracked {
        h.update(f.as_bytes());
        if let Ok(b) = std::fs::read(&f) { h.update(&b); }
    }
    Some(hex::encode(h.finalize()))
}

pub fn read_config() -> Result<Value> {
    let path = super::variant().config;
    let s = std::fs::read_to_string(&path).with_context(|| format!("чтение {path}"))?;
    let mut v: Value = serde_yaml::from_str(&s).with_context(|| format!("разбор {path}"))?;
    if let Some(c) = super::variant().channel { v["security"]["channel"] = json!(c); }
    Ok(v)
}

/// Каталог `experiments/results/<сценарий>/<время>/` и его метаданные.
pub struct Run {
    pub name: String,
    pub dir:  PathBuf,
    meta:     Value,
}

impl Run {
    pub fn new(name: &str, a: &Args, topology: &str, params: Value) -> Result<Self> {
        let var = super::variant();
        let dir = PathBuf::from("experiments/results").join(name)
            .join(format!("{}{}", chrono::Local::now().format("%Y%m%d_%H%M%S"), var.suffix));
        std::fs::create_dir_all(&dir)?;
        let cfg = read_config()?;
        let mut timeouts = serde_json::Map::new();
        for sec in ["dht", "transport", "tunnel"] {
            if let Some(m) = cfg[sec].as_object() {
                for (k, v) in m { if k.ends_with("_ms") { timeouts.insert(k.clone(), v.clone()); } }
            }
        }
        let diff = worktree_hash();
        let meta = json!({
            "scenario": name,
            "driver": "p2p-orchestrator scenario (Rust)",
            "build": if cfg!(debug_assertions) { "debug" } else { "release" },
            "commit": git(&["rev-parse", "HEAD"]),
            "worktree_dirty": diff.is_some(),
            "worktree_diff_sha256": diff,
            "started_at": chrono::Local::now().format("%Y-%m-%dT%H:%M:%S%z").to_string(),
            "timebase": "длительности *_s и *_ms оркестратора — монотонные часы (Instant); *_ms из API узла — часы узла",
            "seed": a.seed,
            "N": a.nodes,
            "k": cfg["dht"]["k_bucket_size"],
            "alpha": cfg["dht"]["alpha"],
            "R": cfg["dht"]["replication_factor"],
            "timeouts_ms": timeouts,
            "tunnel": cfg["tunnel"],
            "bootstrap_graph": topology,
            "channel": cfg["security"]["channel"].as_str().unwrap_or("tls"),
            // соединения между узлами (ADR-39/40/41): пул (что через него идёт) или новое на каждый запрос
            "connections": if cfg["transport"]["reuse_connections"].as_bool().unwrap_or(true) {
                format!("pool:{}", cfg["transport"]["pool_scope"].as_str().unwrap_or("tunnels"))
            } else { "per_request".into() },
            "config_file": var.config,
            "config": cfg,
            "params": params,
            "netem": null,
        });
        Ok(Self { name: name.to_owned(), dir, meta })
    }

    pub fn csv(&self, file: &str, header: &[&str], rows: &[Vec<String>]) -> Result<()> {
        let mut w = csv::Writer::from_path(self.dir.join(file))?;
        w.write_record(header)?;
        for r in rows { w.write_record(r)?; }
        w.flush()?;
        Ok(())
    }

    /// results.csv + meta.json → графики из сохранённых CSV → summary.md.
    pub fn finish(&mut self, header: &[&str], rows: &[Vec<String>], summary: &str) -> Result<()> {
        self.csv("results.csv", header, rows)?;
        self.meta["finished_at"] = json!(chrono::Local::now().format("%Y-%m-%dT%H:%M:%S%z").to_string());
        std::fs::write(self.dir.join("meta.json"), serde_json::to_string_pretty(&self.meta)?)?;
        let charts = super::plot::plot_dir(&self.dir).unwrap_or_else(|e| {
            eprintln!("графики не построены: {e}");
            vec![]
        });
        let m = &self.meta;
        let commit = m["commit"].as_str().unwrap_or("?");
        let dirty = match m["worktree_diff_sha256"].as_str() {
            Some(h) => format!(" + незакоммиченные изменения (diff sha256 `{}`)", &h[..12]),
            None => String::new(),
        };
        let mut md = format!(
            "# {}\n\nкоммит `{commit}`{dirty}, N={}, k={}, α={}, R={}, relay={}, канал={}, bootstrap={}, seed={}, конфиг `{}`; драйвер `p2p-orchestrator scenario`\n\n{summary}\n",
            self.name, m["N"], m["k"], m["alpha"], m["R"], m["tunnel"]["min_relays"], m["channel"].as_str().unwrap_or("tls"),
            m["bootstrap_graph"].as_str().unwrap_or(""), m["seed"], m["config_file"].as_str().unwrap_or(""));
        if !charts.is_empty() {
            md += "\n## Графики (из results.csv)\n\n";
            for (file, caption) in &charts { md += &format!("![{caption}](plots/{file})\n\n"); }
        }
        std::fs::write(self.dir.join("summary.md"), &md)?;
        println!("== {}: {}\n{summary}", self.name, self.dir.display());
        Ok(())
    }
}
