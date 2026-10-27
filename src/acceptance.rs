//! acceptance.rs — воспроизводимая приёмка этапов 1–2 (ТЗ §25) на РЕАЛЬНЫХ измерениях.
//!
//! Сценарий (`p2p-orchestrator acceptance`):
//!  1. запустить seed и остальные узлы (star bootstrap), записать логи;
//!  2. дождаться сходимости (суммарное число контактов меняется ≤10% два окна подряд);
//!  3. выгрузить таблицы маршрутизации всех узлов (JSON) + сводку по bucket (CSV);
//!  4. выполнить контрольные lookup, где цель ОТСУТСТВУЕТ в таблице инициатора;
//!  5. проверить критерии невырожденности (§6 полного ТЗ);
//!  6. остановить bootstrap-узел и повторить lookup + STORE/FIND_VALUE;
//!  7. сохранить сырые данные (CSV/JSON) и сводку. Ничего не генерируется случайно:
//!     пары «инициатор—цель» выбираются детерминированно.

use std::collections::HashSet;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use crate::gui::orch_gui::{Orchestrator, GUI_OFFSET};

pub struct Params {
    pub nodes:    usize,
    pub lookups:  usize,
    pub out:      PathBuf,
    pub config:   PathBuf,
    pub converge_timeout_s: u64,
}

#[derive(Clone)]
struct NodeInfo {
    port:     u16,
    id:       String,
    contacts: HashSet<String>,
    total:    usize,
    buckets:  usize,
    fill:     String,
    routing:  Value,
}

fn now_ms() -> u64 { crate::types::now_ms() }

async fn http(method: &str, port: u16, path: &str, timeout_s: u64) -> Result<Value> {
    let gui = port + GUI_OFFSET;
    let mut s = tokio::time::timeout(Duration::from_secs(3), TcpStream::connect(("127.0.0.1", gui)))
        .await.context("connect timeout")??;
    let req = format!("{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{gui}\r\nConnection: close\r\n\r\n");
    s.write_all(req.as_bytes()).await?;
    let mut buf = Vec::new();
    tokio::time::timeout(Duration::from_secs(timeout_s), s.read_to_end(&mut buf))
        .await.context("response timeout")??;
    let text = String::from_utf8_lossy(&buf);
    let body = text.split_once("\r\n\r\n").map(|x| x.1).unwrap_or("");
    serde_json::from_str(body).with_context(|| format!("bad JSON from :{gui}: {body}"))
}

async fn fetch(port: u16) -> Result<NodeInfo> {
    let st = http("GET", port, "/api/status", 5).await?;
    let routing = st["routing"].clone();
    let mut contacts = HashSet::new();
    let mut fill = Vec::new();
    for b in routing["buckets"].as_array().cloned().unwrap_or_default() {
        let cs = b["contacts"].as_array().cloned().unwrap_or_default();
        fill.push(format!("{}:{}", b["bucket_index"], cs.len()));
        for c in cs { if let Some(id) = c["node_id"].as_str() { contacts.insert(id.to_owned()); } }
    }
    Ok(NodeInfo {
        port,
        id: st["node_id"].as_str().unwrap_or_default().to_owned(),
        total: routing["total_contacts"].as_u64().unwrap_or(0) as usize,
        buckets: routing["non_empty_buckets"].as_u64().unwrap_or(0) as usize,
        fill: fill.join("|"),
        contacts, routing,
    })
}

async fn fetch_all(ports: &[u16]) -> Result<Vec<NodeInfo>> {
    let mut v = Vec::new();
    for &p in ports { v.push(fetch(p).await?); }
    Ok(v)
}

/// min / max / median / mean + число наблюдений; p95 только при n ≥ 20 (ТЗ §15.3).
fn stats(xs: &[f64]) -> Value {
    if xs.is_empty() { return json!({ "n": 0 }); }
    let mut v = xs.to_vec();
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let n = v.len();
    let median = if n % 2 == 1 { v[n / 2] } else { (v[n / 2 - 1] + v[n / 2]) / 2.0 };
    let mean = v.iter().sum::<f64>() / n as f64;
    let mut o = json!({ "n": n, "min": v[0], "max": v[n - 1], "median": median, "mean": mean });
    if n >= 20 { o["p95"] = json!(v[((n as f64 * 0.95).ceil() as usize).min(n) - 1]); }
    o
}

fn write(path: &std::path::Path, s: &str) -> Result<()> {
    if let Some(p) = path.parent() { std::fs::create_dir_all(p)?; }
    std::fs::write(path, s).with_context(|| format!("write {}", path.display()))
}

/// Выполнить серию контрольных lookup. Цель выбирается детерминированно среди узлов,
/// отсутствующих в СВЕЖЕЙ таблице инициатора.
async fn lookup_series(phase: &str, ports: &[u16], count: usize, exclude: &HashSet<u16>) -> Result<Vec<Value>> {
    let mut out = Vec::new();
    let alive: Vec<u16> = ports.iter().copied().filter(|p| !exclude.contains(p)).collect();
    let all = fetch_all(&alive).await?;
    let mut attempts = 0;
    let mut k = 0usize;
    while out.len() < count && attempts < count * 4 {
        attempts += 1;
        let init_port = alive[k % alive.len()];
        k += 1;
        let init = fetch(init_port).await?;
        let shift = k % all.len();
        let target = (0..all.len()).map(|i| &all[(i + shift) % all.len()])
            .find(|t| t.port != init_port && !init.contacts.contains(&t.id));
        let Some(target) = target else { continue };

        let t0 = Instant::now();
        let r = http("POST", init_port, &format!("/api/lookup?target={}", target.id), 40).await?;
        let queried = r["queried_by_iter"].as_array().cloned().unwrap_or_default();
        let queried_nodes: usize = queried.iter().map(|it| it.as_array().map(|a| a.len()).unwrap_or(0)).sum();
        let tshort = &target.id[..8];
        let target_queried = queried.iter().flat_map(|it| it.as_array().cloned().unwrap_or_default())
            .any(|x| x.as_str() == Some(tshort));
        let intermediate = queried_nodes - usize::from(target_queried);
        out.push(json!({
            "phase": phase,
            "ts_ms": now_ms(),
            "initiator_port": init_port,
            "initiator_id": init.id,
            "target_port": target.port,
            "target_id": target.id,
            "target_known_before": r["target_known_before"],
            "found_target": r["found_target"],
            "rpc_count": r["rpc_count"],
            "iterations": r["iterations"],
            "duration_ms": r["duration_ms"],
            "wall_ms": t0.elapsed().as_millis() as u64,
            "queried_nodes": queried_nodes,
            "intermediate_nodes": intermediate,
            "queried_by_iter": r["queried_by_iter"],
            "ok": r["ok"],
        }));
    }
    Ok(out)
}

fn lookups_csv(rows: &[Value]) -> String {
    let mut s = String::from("phase,ts_ms,initiator_port,target_port,target_known_before,found_target,rpc_count,iterations,duration_ms,queried_nodes,intermediate_nodes\n");
    for r in rows {
        s += &format!("{},{},{},{},{},{},{},{},{},{},{}\n",
            r["phase"].as_str().unwrap_or(""), r["ts_ms"], r["initiator_port"], r["target_port"],
            r["target_known_before"], r["found_target"], r["rpc_count"], r["iterations"],
            r["duration_ms"], r["queried_nodes"], r["intermediate_nodes"]);
    }
    s
}

fn lookup_summary(rows: &[Value]) -> Value {
    let qualified: Vec<&Value> = rows.iter().filter(|r| r["target_known_before"] == json!(false)).collect();
    let found = qualified.iter().filter(|r| r["found_target"] == json!(true)).count();
    let with_inter = qualified.iter().filter(|r| r["intermediate_nodes"].as_u64().unwrap_or(0) >= 1).count();
    let col = |k: &str| qualified.iter().filter_map(|r| r[k].as_f64()).collect::<Vec<_>>();
    json!({
        "total_lookups": rows.len(),
        "target_absent_before": qualified.len(),
        "found_target": found,
        "success_rate": if qualified.is_empty() { 0.0 } else { found as f64 / qualified.len() as f64 },
        "used_intermediate_node": with_inter,
        "rpc_count": stats(&col("rpc_count")),
        "iterations": stats(&col("iterations")),
        "duration_ms": stats(&col("duration_ms")),
    })
}

fn git_commit() -> String {
    std::process::Command::new("git").args(["rev-parse", "HEAD"]).output().ok()
        .and_then(|o| String::from_utf8(o.stdout).ok()).map(|s| s.trim().to_owned())
        .unwrap_or_else(|| "unknown".into())
}

/// Возвращает true, если все критерии приёмки выполнены.
pub async fn run(p: Params) -> Result<bool> {
    if p.nodes < 5 { bail!("минимум 5 узлов (ТЗ §25.1)"); }
    std::fs::create_dir_all(&p.out)?;
    // Логи узлов — в общий каталог logs/ (в git не коммитятся), рядом с логами тестов
    let run_name = p.out.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "run".into());
    let log_dir = PathBuf::from("logs").join(&run_name);
    let orch = Orchestrator::new_in(p.config.clone(), p.out.clone(), log_dir.clone());
    let started_ms = now_ms();
    println!("== Приёмка: N={} lookups={} out={}", p.nodes, p.lookups, p.out.display());

    let t_spawn = Instant::now();
    for _ in 0..p.nodes {
        orch.spawn(1).await.map_err(|e| anyhow::anyhow!(e))?;
        tokio::time::sleep(Duration::from_millis(300)).await; // seed поднимается первым
    }
    let ports = orch.ports().await;
    let seed_port = ports[0];

    // ── сходимость ───────────────────────────────────────────────────────────
    let mut prev_sum: Option<usize> = None;
    let mut stable = 0;
    let mut converge_ms = None;
    let deadline = Instant::now() + Duration::from_secs(p.converge_timeout_s);
    tokio::time::sleep(Duration::from_secs(3)).await;
    while Instant::now() < deadline {
        let infos = match fetch_all(&ports).await { Ok(v) => v, Err(_) => { tokio::time::sleep(Duration::from_secs(1)).await; continue } };
        let sum: usize = infos.iter().map(|n| n.total).sum();
        if let Some(prev) = prev_sum {
            if prev > 0 && (sum as f64 - prev as f64).abs() <= 0.10 * prev as f64 { stable += 1 } else { stable = 0 }
        }
        println!("   сходимость: сумма контактов={sum} стабильных окон={stable}");
        prev_sum = Some(sum);
        if stable >= 2 { converge_ms = Some(t_spawn.elapsed().as_millis() as u64); break; }
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
    let converged = converge_ms.is_some();

    // ── выгрузка таблиц (§25.3) ──────────────────────────────────────────────
    let infos = fetch_all(&ports).await?;
    let mut csv = String::from("port,node_id,total_contacts,non_empty_buckets,bucket_fill\n");
    for n in &infos {
        write(&p.out.join(format!("routing_tables/node-{}.json", n.port)),
              &serde_json::to_string_pretty(&n.routing)?)?;
        csv += &format!("{},{},{},{},{}\n", n.port, n.id, n.total, n.buckets, n.fill);
    }
    write(&p.out.join("routing_summary.csv"), &csv)?;

    // ── невырожденность: ≥80% узлов с < N-1 контактов ───────────────────────
    let n = p.nodes;
    let partial = infos.iter().filter(|x| x.total < n - 1).count();
    let partial_share = partial as f64 / n as f64;
    let non_degenerate_tables = partial_share >= 0.80;

    // ── контрольные lookup ───────────────────────────────────────────────────
    println!("== Контрольные lookup (цель отсутствует в таблице инициатора)");
    let before = lookup_series("before_bootstrap_loss", &ports, p.lookups, &HashSet::new()).await?;
    write(&p.out.join("lookups_before.json"), &serde_json::to_string_pretty(&before)?)?;
    write(&p.out.join("lookups_before.csv"), &lookups_csv(&before))?;
    let sum_before = lookup_summary(&before);

    // STORE до отключения bootstrap
    let store_key = "acceptance-key";
    let store_val = "acceptance-value";
    let store_port = ports[1];
    let store_res = http("POST", store_port, &format!("/api/store?key={store_key}&value={store_val}"), 40).await?;

    // ── отказ bootstrap-узла ─────────────────────────────────────────────────
    println!("== Останавливаю bootstrap-узел :{seed_port}");
    orch.kill(seed_port).await;
    tokio::time::sleep(Duration::from_secs(2)).await;
    let mut excl = HashSet::new();
    excl.insert(seed_port);
    let after = lookup_series("after_bootstrap_loss", &ports, p.lookups.min(10).max(10), &excl).await?;
    write(&p.out.join("lookups_after_bootstrap_loss.json"), &serde_json::to_string_pretty(&after)?)?;
    write(&p.out.join("lookups_after_bootstrap_loss.csv"), &lookups_csv(&after))?;
    let sum_after = lookup_summary(&after);

    // FIND_VALUE после отказа bootstrap, с узла, не участвовавшего в STORE
    let find_port = *ports.last().unwrap();
    let find_res = http("GET", find_port, &format!("/api/find?key={store_key}"), 40).await?;
    let find_ok = find_res["found"] == json!(true) && find_res["value"] == json!(store_val);

    // ── сводка ───────────────────────────────────────────────────────────────
    let ok_before = sum_before["target_absent_before"].as_u64().unwrap_or(0) >= p.lookups.min(30) as u64
        && sum_before["success_rate"].as_f64().unwrap_or(0.0) >= 0.95
        && sum_before["used_intermediate_node"] == sum_before["target_absent_before"];
    let ok_after = sum_after["target_absent_before"].as_u64().unwrap_or(0) >= 10
        && sum_after["success_rate"].as_f64().unwrap_or(0.0) >= 0.95;
    let checks = json!({
        "converged": converged,
        "tables_non_degenerate_ge80pct_below_N-1": non_degenerate_tables,
        "lookups_before_ge_required_and_95pct_success_and_intermediate": ok_before,
        "lookups_after_bootstrap_loss_ok": ok_after,
        "find_value_after_bootstrap_loss": find_ok,
    });
    let all_ok = checks.as_object().unwrap().values().all(|v| v == &json!(true));

    let k = infos.first().and_then(|x| x.routing["k"].as_u64()).unwrap_or(0);
    let summary = json!({
        "params": { "N": n, "k": k, "lookups_requested": p.lookups,
                    "config": p.config, "git_commit": git_commit(),
                    "started_ms": started_ms, "finished_ms": now_ms(),
                    "bootstrap": "star", "bootstrap_node_port": seed_port,
                    "pair_selection": "детерминированная (по кругу), цель отсутствует в свежей таблице инициатора",
                    "seed": null },
        "convergence_ms": converge_ms,
        "tables": { "nodes_with_lt_N-1_contacts": partial, "share": partial_share,
                    "contacts": stats(&infos.iter().map(|x| x.total as f64).collect::<Vec<_>>()) },
        "lookups_before_bootstrap_loss": sum_before,
        "lookups_after_bootstrap_loss": sum_after,
        "store": store_res,
        "find_value_after_bootstrap_loss": find_res,
        "checks": checks,
        "all_ok": all_ok,
    });
    write(&p.out.join("summary.json"), &serde_json::to_string_pretty(&summary)?)?;

    let mut md = format!("# Результаты приёмки этапов 1–2\n\nN={n}, k={k}, коммит `{}`.\n\n| Критерий | Результат |\n|---|---|\n", git_commit());
    for (kname, v) in checks.as_object().unwrap() {
        md += &format!("| {kname} | {} |\n", if v == &json!(true) { "✅" } else { "❌" });
    }
    md += &format!("\nСходимость: {} мс. Узлов с < N-1 контактами: {partial}/{n}.\n\
        Lookup до отказа: {}\nLookup после отказа bootstrap: {}\n",
        converge_ms.map(|x| x.to_string()).unwrap_or("не достигнута".into()),
        serde_json::to_string(&sum_before)?, serde_json::to_string(&sum_after)?);
    write(&p.out.join("summary.md"), &md)?;

    for port in orch.ports().await { orch.kill(port).await; }
    // Закрытые ключи узлов НЕ должны попасть в репозиторий (ТЗ §22): удаляем state/
    let _ = std::fs::remove_dir_all(p.out.join("state"));
    println!("{md}");
    println!("== Данные: {}  Логи узлов: {}", p.out.display(), log_dir.display());
    Ok(all_ok)
}
