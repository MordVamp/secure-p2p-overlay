//! experiment/scenarios.rs — сценарии 01–09 ТЗ §15.1 и проверка pcap на реальных процессах.
//!
//! Отказы — SIGKILL; после инъекции узел перезапускается с той же identity, чтобы условия
//! следующей инъекции были одинаковыми. Случайный выбор пар — от `--seed`.

use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::{bail, Result};
use rand::rngs::StdRng;
use rand::seq::SliceRandom;
use rand::{Rng, SeedableRng};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::net::Net;
use super::report::{f1, fmt, jv, stat, Run};
use super::Args;

pub const ALL: [&str; 9] = ["01", "02", "03", "04", "05", "06", "07", "08", "09"];

/// Выполнить сценарий (`01`…`09`, `pcap`) или все 01–09 (`all`). false — проверка сценария не пройдена.
pub async fn run(which: &str, a: &Args) -> Result<bool> {
    if !Path::new(super::report::CONFIG).exists() {
        bail!("запускайте из корня репозитория (нет {})", super::report::CONFIG);
    }
    if a.nodes < 6 { bail!("нужно ≥6 узлов: отправитель, 2 relay, получатель и запас на отказы"); }
    super::set_variant(a);
    if !Path::new(&super::variant().config).exists() { bail!("нет конфига {}", super::variant().config); }
    let list: Vec<&str> = if which == "all" { ALL.to_vec() } else { vec![which] };
    let mut ok = true;
    for w in list {
        println!("\n### сценарий {w}");
        ok &= match w {
            "01" => s01(a).await?,
            "02" => s02(a).await?,
            "03" => s03(a).await?,
            "04" => s04(a).await?,
            "05" => s05(a).await?,
            "06" => s06(a).await?,
            "07" => s07(a).await?,
            "08" => s08(a).await?,
            "09" => s09(a).await?,
            "pcap" => spcap(a).await?,
            "channel" => schannel(a).await?,
            "files" => sfiles(a).await?,
            "failover" => sfailover(a).await?,
            "strategy" => sstrategy(a).await?,
            "10" => s10_netem(a).await?,
            "underlay" => sunderlay(a).await?,
            "media" => smedia(a).await?,
            "call" => scall(a).await?,
            "voice" => svoice(a).await?,
            other => bail!("неизвестный сценарий {other}; есть 01–09, pcap, channel, files, failover, strategy, media, call, 10, underlay, all"),
        };
    }
    Ok(ok)
}

fn s<T: ToString>(x: T) -> String { x.to_string() }

fn ms_since(t0: Instant) -> f64 { t0.elapsed().as_secs_f64() * 1000.0 }

fn is_true(v: &Value) -> bool { v.as_bool() == Some(true) }

async fn sleep_s(x: f64) { tokio::time::sleep(Duration::from_secs_f64(x)).await }

/// Случайный индекс из 0..n, отличный от `i`.
fn other(rng: &mut StdRng, n: usize, i: usize) -> usize {
    let t = rng.gen_range(0..n - 1);
    if t >= i { t + 1 } else { t }
}

async fn lookup(net: &Net, i: usize, target: &str) -> Value {
    net.api(i, "POST", "/api/lookup", &[("target", target)]).await
        .unwrap_or_else(|e| json!({ "ok": false, "error": e.to_string() }))
}

async fn send(net: &Net, from: usize, to: usize, text: &str) -> Value {
    let id = net.id(to);
    net.api(from, "POST", "/api/send", &[("target", &id), ("text", text)]).await
        .unwrap_or_else(|e| json!({ "ok": false, "delivered": false, "error": e.to_string() }))
}

/// Остановить узлы и сохранить их журналы в результаты прогона.
async fn close(net: &mut Net, dst: &Path) -> Result<()> {
    net.stop().await;
    net.save_logs(dst)
}

// ── 01 / 09: операционная сходимость (ТЗ §15.1) ──────────────────────────────

const ROUND_PAUSE_S: f64 = 1.0;
const CONVERGE_TIMEOUT_S: f64 = 120.0;

struct Round { t_s: f64, success: f64, csum: i64, cs: Vec<i64> }

/// Контрольные раунды по заранее сформированному набору пар (узлам он не передаётся).
/// Сходимость — два раунда подряд с ≥95% успешных lookup и изменением суммы контактов ≤10%;
/// время — от запуска первого процесса до конца второго подтверждающего раунда.
async fn op_converge(net: &mut Net, rng: &mut StdRng) -> (bool, f64, Vec<Round>) {
    let n = net.n();
    let plan: Vec<Vec<(usize, usize)>> = (0..200)
        .map(|_| (0..n).map(|i| (i, other(rng, n, i))).collect())
        .collect();
    let mut rounds: Vec<Round> = vec![];
    for pairs in plan {
        let mut ok = 0;
        for &(i, t) in &pairs {
            let r = lookup(net, i, &net.id(t)).await;
            if is_true(&r["ok"]) && is_true(&r["found_target"]) { ok += 1; }
        }
        let mut cs = vec![];
        for i in 0..n { cs.push(net.contacts(i).await); }
        let cur = Round {
            t_s: net.t_spawn.elapsed().as_secs_f64(),
            success: ok as f64 / pairs.len() as f64,
            csum: cs.iter().map(|c| (*c).max(0)).sum(),
            cs,
        };
        let done = rounds.last().is_some_and(|p| p.success >= 0.95 && cur.success >= 0.95
            && (cur.csum - p.csum).abs() as f64 <= 0.10 * p.csum.max(1) as f64);
        let t = cur.t_s;
        rounds.push(cur);
        if done { return (true, t, rounds); }
        if t > CONVERGE_TIMEOUT_S { break; }
        sleep_s(ROUND_PAUSE_S).await;
    }
    (false, f64::NAN, rounds)
}

const COLD_HDR: [&str; 12] = ["topology", "run", "converged", "converge_s", "rounds", "launch_span_s",
    "last_round_success", "contacts_sum", "contacts_min", "contacts_max", "join_ms_median", "join_ms_max"];

#[derive(Default)]
struct Cold { rows: Vec<Vec<String>>, rounds: Vec<Vec<String>>, joins: Vec<Vec<String>> }

async fn cold_runs(a: &Args, rng: &mut StdRng, topo: &str, run_dir: &Path, c: &mut Cold) -> Result<()> {
    for k in 1..=a.runs {
        let mut net = Net::start(&format!("cold_{topo}"), a.nodes, topo).await?;
        let (ok, t, rounds) = op_converge(&mut net, rng).await;
        for (j, r) in rounds.iter().enumerate() {
            c.rounds.push(vec![s(topo), s(k), s(j), f1(r.t_s), format!("{:.3}", r.success), s(r.csum)]);
        }
        let mut joins = vec![];
        for i in 1..net.n() {   // узел 0 — seed, bootstrap не выполняет
            if let Ok(st) = net.status(i).await {
                if let Some(j) = st["bootstrap"]["join_ms"].as_f64() {
                    joins.push(j);
                    c.joins.push(vec![s(topo), s(k), s(i), s(j)]);
                }
            }
        }
        let last = rounds.last();
        let cs = last.map(|r| r.cs.clone()).unwrap_or_default();
        let js = stat(&joins);
        c.rows.push(vec![
            s(topo), s(k), s(ok), if ok { f1(t) } else { String::new() }, s(rounds.len()), f1(net.launch_span_s),
            last.map(|r| format!("{:.3}", r.success)).unwrap_or_default(),
            s(cs.iter().map(|x| (*x).max(0)).sum::<i64>()),
            cs.iter().min().map(s).unwrap_or_default(), cs.iter().max().map(s).unwrap_or_default(),
            f1(js.median), f1(js.max),
        ]);
        if k == 1 { close(&mut net, &run_dir.join("node_logs").join(format!("{topo}_run1"))).await?; }
        else { net.stop().await; }
    }
    Ok(())
}

fn cold_line(c: &Cold, topo: &str) -> String {
    let sub: Vec<&Vec<String>> = c.rows.iter().filter(|r| r[0] == topo).collect();
    let conv: Vec<f64> = sub.iter().filter(|r| r[2] == "true").filter_map(|r| r[3].parse().ok()).collect();
    let mins: Vec<i64> = sub.iter().filter_map(|r| r[8].parse().ok()).collect();
    let maxs: Vec<i64> = sub.iter().filter_map(|r| r[9].parse().ok()).collect();
    let sums: Vec<f64> = sub.iter().filter_map(|r| r[7].parse().ok()).collect();
    let joins: Vec<f64> = c.joins.iter().filter(|j| j[0] == topo).filter_map(|j| j[3].parse().ok()).collect();
    format!("- **{topo}**: сошлось {}/{}; время операционной сходимости, с: {}; контактов на узел (min…max по прогонам): {}…{}, \
             среднее суммарное {:.1}\n  - bootstrap узла (часы узла: от старта процесса до конца PING bootstrap-узла + self-lookup), мс: {}",
        conv.len(), sub.len(), fmt(&conv), mins.iter().min().unwrap_or(&0), maxs.iter().max().unwrap_or(&0),
        stat(&sums).mean, fmt(&joins))
}

fn cold_method(a: &Args) -> String {
    format!("Критерий §15.1: два контрольных раунда подряд (по {} lookup из заранее сформированного набора) с успехом ≥95% \
             и изменением суммы контактов ≤10% (окно наблюдения — пауза {ROUND_PAUSE_S} с между раундами); отсчёт — от запуска \
             первого процесса. Процессы запускаются с шагом 0.15 с (launch_span_s), поэтому нижняя граница метода ≈ \
             launch_span_s + 2 раунда + окно.", a.nodes)
}

async fn s01(a: &Args) -> Result<bool> {
    let mut run = Run::new("01_cold_start", a, "star", json!({ "runs": a.runs, "round_pause_s": ROUND_PAUSE_S }))?;
    let mut rng = StdRng::seed_from_u64(a.seed);
    let mut c = Cold::default();
    cold_runs(a, &mut rng, "star", &run.dir, &mut c).await?;
    run.csv("rounds.csv", &["topology", "run", "round", "t_s", "success", "contacts_sum"], &c.rounds)?;
    run.csv("join.csv", &["topology", "run", "node", "join_ms"], &c.joins)?;
    run.finish(&COLD_HDR, &c.rows, &format!(
        "Холодный запуск, {} независимых запусков. {}\n\n{}\n\nПо раундам — rounds.csv, bootstrap каждого узла — join.csv, \
         журналы узлов первого запуска — node_logs/.", a.runs, cold_method(a), cold_line(&c, "star")))?;
    Ok(true)
}

async fn s09(a: &Args) -> Result<bool> {
    // §14: упрощённый уровень — star и ring; продвинутый добавляет tree и multi-seed
    let advanced = super::report::read_config()?["level"].as_str() == Some("advanced");
    let topos: &[&str] = if advanced { &["star", "ring", "tree", "multiseed"] } else { &["star", "ring"] };
    let mut run = Run::new("09_bootstrap_compare", a, &topos.join("+"), json!({ "runs": a.runs, "round_pause_s": ROUND_PAUSE_S, "schemes": topos }))?;
    let mut rng = StdRng::seed_from_u64(a.seed);
    let mut c = Cold::default();
    for topo in topos { cold_runs(a, &mut rng, topo, &run.dir, &mut c).await?; }
    run.csv("rounds.csv", &["topology", "run", "round", "t_s", "success", "contacts_sum"], &c.rounds)?;
    run.csv("join.csv", &["topology", "run", "node", "join_ms"], &c.joins)?;
    run.finish(&COLD_HDR, &c.rows, &format!(
        "Сравнение bootstrap-схем при прочих равных (N, k, α, R, тайм-ауты, seed): star — все узлы к узлу 0, ring — узел i \
         к узлу i−1{}. {} запусков на схему. {}\n\n{}",
        if advanced { ", tree — двоичное дерево (к родителю (i−1)/2), multiseed — три seed, остальные знают все три" } else { "" },
        a.runs, cold_method(a), topos.iter().map(|t| cold_line(&c, t)).collect::<Vec<_>>().join("\n")))?;
    Ok(true)
}

// ── 02: FIND_NODE для цели, отсутствующей у инициатора ───────────────────────

async fn s02(a: &Args) -> Result<bool> {
    let mut run = Run::new("02_find_node", a, "star", json!({ "lookups": a.n }))?;
    let mut rng = StdRng::seed_from_u64(a.seed);
    let mut net = Net::start("s02", a.nodes, "star").await?;
    net.converge(60.0, 3).await;
    let n = net.n();
    let (mut rows, mut durs, mut rpcs, mut its) = (vec![], vec![], vec![], vec![]);
    for k in 0..a.n {
        // цель — узел, которого нет в текущей таблице инициатора (по данным самого узла)
        let mut pick = None;
        for _ in 0..50 {
            let i = rng.gen_range(0..n);
            let known = net.contact_ids(i).await.unwrap_or_default();
            let cands: Vec<usize> = (0..n).filter(|&j| j != i && !known.contains(&net.nodes[j].id)).collect();
            if let Some(&t) = cands.choose(&mut rng) { pick = Some((i, t)); break; }
        }
        let (i, t) = pick.unwrap_or_else(|| { let i = rng.gen_range(0..n); (i, other(&mut rng, n, i)) });
        let r = lookup(&net, i, &net.id(t)).await;
        let ok = is_true(&r["ok"]) && is_true(&r["found_target"]);
        rows.push(vec![s(k), s(i), s(t), net.id(t)[..8].to_owned(), jv(&r["target_known_before"]), s(ok),
                       jv(&r["rpc_count"]), jv(&r["iterations"]), jv(&r["duration_ms"])]);
        if ok {
            durs.extend(r["duration_ms"].as_f64());
            rpcs.extend(r["rpc_count"].as_f64());
            its.extend(r["iterations"].as_f64());
        }
    }
    close(&mut net, &run.dir.join("node_logs")).await?;
    let found = rows.iter().filter(|r| r[5] == "true").count();
    let absent = rows.iter().filter(|r| r[4] == "false").count();
    run.finish(&["n", "initiator", "target", "target_id", "known_before", "found", "rpc_count", "iterations", "duration_ms"], &rows,
        &format!("Lookup {}; цель отсутствовала в таблице инициатора (по данным узла): {absent}/{}; найдено {found}/{} ({}%).\n\n\
                  - duration_ms: {}\n- RPC на lookup: {}\n- итераций: {}",
                 rows.len(), rows.len(), rows.len(), 100 * found / rows.len().max(1), fmt(&durs), fmt(&rpcs), fmt(&its)))?;
    Ok(true)
}

// ── 03: STORE / FIND_VALUE, R=3 ──────────────────────────────────────────────

/// Узлы, у которых в хранилище есть запись (ключ DHT = SHA-256 строки ключа).
async fn holders(net: &mut Net, key: &str) -> Vec<usize> {
    let kh = hex::encode(Sha256::digest(key.as_bytes()));
    let mut out = vec![];
    for i in net.alive() {
        if let Ok(st) = net.status(i).await {
            if st["store"]["records"].as_array().into_iter().flatten().any(|r| r["key"].as_str() == Some(kh.as_str())) {
                out.push(i);
            }
        }
    }
    out
}

fn short_uuid() -> String { uuid::Uuid::new_v4().simple().to_string()[..8].to_owned() }

async fn s03(a: &Args) -> Result<bool> {
    let mut run = Run::new("03_store_find_value", a, "star", json!({ "operations": a.n }))?;
    let mut rng = StdRng::seed_from_u64(a.seed);
    let mut net = Net::start("s03", a.nodes, "star").await?;
    net.converge(60.0, 3).await;
    let n = net.n();
    let (mut rows, mut st, mut fd, mut fd_remote) = (vec![], vec![], vec![], vec![]);
    for k in 0..a.n {
        let w = rng.gen_range(0..n);
        let r = other(&mut rng, n, w);
        let (key, val) = (format!("s03-{k}-{}", short_uuid()), format!("v{k}"));
        let t0 = Instant::now();
        let sres = net.api(w, "POST", "/api/store", &[("key", &key), ("value", &val)]).await.unwrap_or_else(|e| json!({ "ok": false, "error": e.to_string() }));
        let tw = ms_since(t0);
        let hs = holders(&mut net, &key).await;
        let local = hs.contains(&r);
        let t0 = Instant::now();
        let f = net.api(r, "GET", "/api/find", &[("key", &key)]).await.unwrap_or_else(|_| json!({ "found": false }));
        let tr = ms_since(t0);
        let ok = is_true(&f["found"]) && f["value"].as_str() == Some(val.as_str());
        rows.push(vec![s(k), s(w), s(r), jv(&sres["ok"]), jv(&sres["stored_on"]), s(hs.len()), s(local), f1(tw), s(ok), f1(tr)]);
        if is_true(&sres["ok"]) { st.push(tw); }
        if ok { fd.push(tr); if !local { fd_remote.push(tr); } }
    }
    close(&mut net, &run.dir.join("node_logs")).await?;
    let okc = rows.iter().filter(|x| x[8] == "true").count();
    let reps: Vec<f64> = rows.iter().filter_map(|x| x[5].parse().ok()).collect();
    run.finish(&["n", "writer", "reader", "store_ok", "stored_on", "holders", "reader_is_holder", "store_ms", "found_ok", "find_ms"], &rows,
        &format!("Операций {}; STORE успешен {}; значение прочитано верно {okc}/{}.\nstored_on — число подтвердивших узлов включая автора \
                  (R=3 реплики + локальная копия); holders — фактически найдено в хранилищах по /api/status: {}.\n\n\
                  - STORE, мс (lookup ключа + 3×STORE): {}\n- FIND_VALUE, мс (все): {}\n- FIND_VALUE, мс (читатель не хранитель): {}",
                 rows.len(), st.len(), rows.len(), fmt(&reps), fmt(&st), fmt(&fd), fmt(&fd_remote)))?;
    Ok(true)
}

// ── 04: отказ хранителя ──────────────────────────────────────────────────────

async fn s04(a: &Args) -> Result<bool> {
    let plan: Vec<usize> = std::iter::repeat(1).take(a.inj).chain([2, 2, 2, 3, 3, 3]).collect();
    let mut run = Run::new("04_keeper_failure", a, "star", json!({ "injections": plan }))?;
    let mut rng = StdRng::seed_from_u64(a.seed);
    let mut net = Net::start("s04", a.nodes, "star").await?;
    net.converge(60.0, 3).await;
    let writer = 0;
    let mut rows = vec![];
    for (k, &kill_n) in plan.iter().enumerate() {
        let (key, val) = (format!("s04-{k}-{}", short_uuid()), format!("keep{k}"));
        let sres = net.api(writer, "POST", "/api/store", &[("key", &key), ("value", &val)]).await.unwrap_or_else(|_| json!({}));
        let hs: Vec<usize> = holders(&mut net, &key).await.into_iter().filter(|&h| h != writer).collect();
        let victims: Vec<usize> = hs.choose_multiple(&mut rng, kill_n.min(hs.len())).copied().collect();
        for &v in &victims { net.kill(v).await; }
        sleep_s(1.0).await;
        let readers: Vec<usize> = net.alive().into_iter().filter(|i| *i != writer && !hs.contains(i)).collect();
        let Some(&r) = readers.choose(&mut rng) else { bail!("04: нет живого узла-читателя") };
        let t0 = Instant::now();
        let f = net.api(r, "GET", "/api/find", &[("key", &key)]).await.unwrap_or_else(|_| json!({ "found": false }));
        let ms = ms_since(t0);
        let ok = is_true(&f["found"]) && f["value"].as_str() == Some(val.as_str());
        rows.push(vec![s(k), s(kill_n), jv(&sres["stored_on"]), s(hs.len()), s(victims.len()), s(hs.len() - victims.len()), s(r), s(ok), f1(ms)]);
        for &v in &victims { net.restart(v).await?; }
        net.converge(20.0, 2).await;
    }
    close(&mut net, &run.dir.join("node_logs")).await?;
    let mut lines = vec![];
    for kn in 1..=3 {
        let sub: Vec<&Vec<String>> = rows.iter().filter(|x| x[1] == s(kn)).collect();
        if sub.is_empty() { continue; }
        let okd: Vec<f64> = sub.iter().filter(|x| x[7] == "true").filter_map(|x| x[8].parse().ok()).collect();
        lines.push(format!("- отключено хранителей {kn}: найдено {}/{}; find_ms: {}", okd.len(), sub.len(), fmt(&okd)));
    }
    run.finish(&["n", "kill_planned", "stored_on", "holders_excl_writer", "killed", "holders_left", "reader", "found_ok", "find_ms"], &rows,
        &format!("Запись публикует узел 0; отключаются (SIGKILL) хранители-реплики (автор не трогается), читает живой узел, не хранящий \
                  запись. После каждой инъекции отключённые узлы перезапускаются (та же identity) — условия следующей инъекции одинаковы. \
                  Основной сценарий §15.1 п.4 — отключение одного хранителя; 2 и 3 — граница отказоустойчивости при R=3.\n\n{}", lines.join("\n")))?;
    Ok(true)
}

// ── 05: отказ промежуточных кандидатов lookup ────────────────────────────────

async fn s05(a: &Args) -> Result<bool> {
    let fracs = [0.0, 0.2, 0.4];
    let mut run = Run::new("05_lookup_failure", a, "star", json!({ "fail_fractions": fracs, "lookups_per_level": a.n, "wait_after_kill_s": 2 }))?;
    let mut rng = StdRng::seed_from_u64(a.seed);
    let mut net = Net::start("s05", a.nodes, "star").await?;
    net.converge(60.0, 3).await;
    let n = net.n();
    let mut order: Vec<usize> = (1..n).collect();
    order.shuffle(&mut rng);
    let mut rows = vec![];
    for frac in fracs {
        let killed = &order[..(frac * n as f64) as usize];
        for &v in killed { if net.is_alive(v) { net.kill(v).await; } }
        sleep_s(2.0).await;   // мёртвые узлы ещё в таблицах маршрутизации — lookup натыкается на них
        let al = net.alive();
        for k in 0..a.n {
            let i = *al.choose(&mut rng).unwrap();
            let t = *al.iter().filter(|&&j| j != i).collect::<Vec<_>>().choose(&mut rng).copied().unwrap();
            let r = lookup(&net, i, &net.id(t)).await;
            rows.push(vec![s(frac), s(killed.len()), s(al.len()), s(k), s(i), s(t), s(is_true(&r["ok"]) && is_true(&r["found_target"])),
                           jv(&r["rpc_count"]), jv(&r["iterations"]), jv(&r["duration_ms"])]);
        }
    }
    close(&mut net, &run.dir.join("node_logs")).await?;
    let mut lines = vec![];
    for frac in fracs {
        let sub: Vec<&Vec<String>> = rows.iter().filter(|x| x[0] == s(frac)).collect();
        let ok: Vec<&&Vec<String>> = sub.iter().filter(|x| x[6] == "true").collect();
        let d: Vec<f64> = ok.iter().filter_map(|x| x[9].parse().ok()).collect();
        let r: Vec<f64> = ok.iter().filter_map(|x| x[7].parse().ok()).collect();
        lines.push(format!("- отключено {} из {n} ({}%): успех {}/{}; duration_ms {}; RPC {}",
            sub[0][1], (frac * 100.0) as u32, ok.len(), sub.len(), fmt(&d), fmt(&r)));
    }
    run.finish(&["fail_fraction", "killed", "alive", "n", "initiator", "target", "found", "rpc_count", "iterations", "duration_ms"], &rows,
        &format!("Часть узлов останавливается (SIGKILL), через 2 с — пока они ещё числятся в таблицах маршрутизации и попадают в кандидаты \
                  lookup — живые узлы ищут друг друга. Отключения накопительные.\n\n{}", lines.join("\n")))?;
    Ok(true)
}

// ── 06: туннель и сообщения (первое сообщение vs установленная сессия) ────────

async fn s06(a: &Args) -> Result<bool> {
    let per_pair = 3;
    let mut run = Run::new("06_tunnel_message", a, "star", json!({ "pairs": a.n, "messages_per_pair": per_pair }))?;
    let mut rng = StdRng::seed_from_u64(a.seed);
    let mut net = Net::start("s06", a.nodes, "star").await?;
    net.converge(60.0, 3).await;
    let n = net.n();
    let mut pairs: Vec<(usize, usize)> = (0..n).flat_map(|a| (a + 1..n).map(move |b| (a, b))).collect();
    pairs.shuffle(&mut rng);
    let mut rows = vec![];
    for (p, &(x, y)) in pairs.iter().take(a.n).enumerate() {
        let (from, to) = if rng.gen_bool(0.5) { (y, x) } else { (x, y) };
        for m in 0..per_pair {
            let r = send(&net, from, to, &format!("s06-{p}-{m}")).await;
            let kind = match r["tunnel_reused"].as_bool() { Some(true) => "session", Some(false) => "first", None => "" };
            rows.push(vec![s(p), s(m), s(from), s(to), s(kind), s(is_true(&r["delivered"])), jv(&r["rtt_ms"]), jv(&r["lookup_ms"]),
                           jv(&r["tunnel_build_ms"]), jv(&r["tunnel_send_ms"]), jv(&r["rebuilt_after_failure"]), jv(&r["error"])]);
        }
    }
    close(&mut net, &run.dir.join("node_logs")).await?;
    let col = |kind: &str, c: usize| -> Vec<f64> {
        rows.iter().filter(|x| x[5] == "true" && x[4] == kind).filter_map(|x| x[c].parse().ok()).collect()
    };
    let delivered = rows.iter().filter(|x| x[5] == "true").count();
    run.finish(&["pair", "msg", "sender", "receiver", "kind", "delivered", "rtt_ms", "lookup_ms", "tunnel_build_ms", "tunnel_send_ms", "rebuilt", "error"], &rows,
        &format!("{} пар (неупорядоченные пары не повторяются, туннелей между ними до первого сообщения нет), по {per_pair} сообщения на пару. \
                  Доставлено с APP_ACK {delivered}/{}.\nfirst — туннель строился в этой отправке (lookup + TUNNEL_BUILD; у получателя — свой \
                  обратный туннель для APP_ACK); session — туннель уже установлен, lookup не выполняется.\n\n\
                  - время построения туннеля, мс: {}\n- lookup получателя (first), мс: {}\n- первое сообщение, RTT до APP_ACK, мс: {}\n\
                  - установленная сессия, RTT до APP_ACK, мс: {}\n- установленная сессия, проход по цепочке до получателя (TUNNEL_ACK), мс: {}",
                 a.n.min(pairs.len()), rows.len(), fmt(&col("first", 8)), fmt(&col("first", 7)), fmt(&col("first", 6)),
                 fmt(&col("session", 6)), fmt(&col("session", 9))))?;
    Ok(true)
}

// ── 07: отказ активного ретранслятора ────────────────────────────────────────

async fn route_of(net: &Net, from: usize, to: usize) -> Vec<String> {
    let id = net.id(to);
    match net.api(from, "GET", "/api/route", &[("target", &id)]).await {
        Ok(v) => v["tunnel"]["hops"].as_array().into_iter().flatten()
            .filter_map(|h| h["addr"].as_str().map(str::to_owned)).collect(),
        Err(_) => vec![],
    }
}

async fn s07(a: &Args) -> Result<bool> {
    let mut run = Run::new("07_relay_failure", a, "star", json!({ "injections": a.inj, "recovery_limit_s": 60 }))?;
    let mut rng = StdRng::seed_from_u64(a.seed);
    let mut net = Net::start("s07", a.nodes, "star").await?;
    net.converge(60.0, 3).await;
    let mut rows = vec![];
    for k in 0..a.inj {
        let alive = net.alive();
        let pair: Vec<usize> = alive.choose_multiple(&mut rng, 2).copied().collect();
        let (from, to) = (pair[0], pair[1]);
        let warm = send(&net, from, to, &format!("s07-warm-{k}")).await;
        let hops = route_of(&net, from, to).await;
        let relays: Vec<usize> = hops.iter().filter_map(|h| (0..net.n()).find(|&i| net.addr(i) == *h)).collect();
        if !is_true(&warm["delivered"]) || relays.is_empty() {
            rows.push(vec![s(k), s(from), s(to), String::new(), String::new(), s(is_true(&warm["delivered"])),
                           String::new(), String::new(), String::new(), String::new(), String::new(), s("no-active-tunnel")]);
            continue;
        }
        let pos = k % relays.len();
        let victim = relays[pos];
        net.kill(victim).await;
        let t0 = Instant::now();
        let (mut fails, mut res) = (0, json!({}));
        while t0.elapsed().as_secs_f64() < 60.0 {
            res = send(&net, from, to, &format!("s07-after-{k}-{fails}")).await;
            if is_true(&res["delivered"]) { break; }
            fails += 1;
        }
        let recovered = is_true(&res["delivered"]);
        let rec = recovered.then(|| ms_since(t0));
        let avoided = !route_of(&net, from, to).await.contains(&net.addr(victim));
        rows.push(vec![s(k), s(from), s(to), s(victim), s(pos), s(true), s(fails), rec.map(f1).unwrap_or_default(),
                       jv(&res["rebuilt_after_failure"]), jv(&res["tunnel_build_ms"]), s(avoided),
                       s(if recovered { "recovered" } else { "not-recovered" })]);
        net.restart(victim).await?;
        net.converge(20.0, 2).await;
    }
    close(&mut net, &run.dir.join("node_logs")).await?;
    let okr: Vec<&Vec<String>> = rows.iter().filter(|x| x[11] == "recovered").collect();
    let inj = rows.iter().filter(|x| !x[3].is_empty()).count();
    let rec: Vec<f64> = okr.iter().filter_map(|x| x[7].parse().ok()).collect();
    let build: Vec<f64> = okr.iter().filter_map(|x| x[9].parse().ok()).collect();
    run.finish(&["n", "sender", "receiver", "killed_relay", "hop_index", "delivered_before", "failed_attempts", "recovery_ms",
                 "rebuilt_in_send", "rebuild_ms", "new_route_avoids_killed", "outcome"], &rows,
        &format!("Отправитель строит туннель (сообщение доставлено), затем relay этого туннеля отключается (SIGKILL; hop_index чередуется), \
                  и сообщение отправляется повторно до доставки (лимит 60 с). recovery_ms — от отключения до APP_ACK; включает обнаружение \
                  отказа (нет TUNNEL_ACK), lookup, построение нового туннеля без отказавшего relay и повтор. После инъекции relay перезапускается.\n\n\
                  - инъекций {inj}, доставка восстановлена {}/{inj}; без единой неудачной попытки (перестроение внутри той же отправки): {}\n\
                  - новый маршрут не содержит отключённый relay: {}/{}\n- время восстановления доставки, мс: {}\n\
                  - из них построение нового туннеля, мс: {}",
                 okr.len(), okr.iter().filter(|x| x[6] == "0").count(), okr.iter().filter(|x| x[10] == "true").count(), okr.len(),
                 fmt(&rec), fmt(&build)))?;
    Ok(okr.len() == inj)
}

// ── 08: негативные криптографические и replay-тесты ──────────────────────────

async fn s08(a: &Args) -> Result<bool> {
    let mut run = Run::new("08_negative_crypto", a, "-", json!({}))?;
    let out = tokio::process::Command::new("cargo").args(["test", "--tests"]).output().await?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    let tests: Vec<(String, String)> = stdout.lines()
        .filter_map(|l| l.strip_prefix("test ")?.split_once(" ... "))
        .map(|(n, st)| (n.to_owned(), st.split_whitespace().next().unwrap_or("").to_owned()))
        .collect();
    const NEG: [&str; 19] = ["neg", "bad", "wrong", "replay", "tamper", "reject", "forged", "expired", "mismatch", "foreign",
        "stale", "rollback", "invalid", "unknown", "oversize", "too_large", "loop", "fail", "attack"];
    let sel: Vec<Vec<String>> = tests.iter().filter(|(n, _)| NEG.iter().any(|p| n.contains(p)))
        .map(|(n, st)| vec![n.clone(), st.clone()]).collect();
    let cwd = std::env::current_dir()?.display().to_string();
    let log = format!("{stdout}{}", String::from_utf8_lossy(&out.stderr)).replace(&cwd, ".");
    std::fs::write(run.dir.join("cargo_test.txt"), log)?;
    let ok_all = tests.iter().filter(|(_, st)| st == "ok").count();
    run.finish(&["test", "status"], &sel,
        &format!("Реальный вывод `cargo test --tests`: всего тестов {}, из них ok {ok_all}; негативных (по имени) {}, из них ok {}. \
                  Полный вывод — cargo_test.txt.", tests.len(), sel.len(), sel.iter().filter(|r| r[1] == "ok").count()))?;
    Ok(out.status.success() && ok_all == tests.len() && !tests.is_empty())
}

// ── pcap: в захвате оверлейного трафика нет открытого текста ──────────────────

fn which(name: &str) -> Option<std::path::PathBuf> {
    std::env::var_os("PATH").and_then(|p| std::env::split_paths(&p).map(|d| d.join(name)).find(|f| f.is_file()))
}

async fn spcap(a: &Args) -> Result<bool> {
    let (tool, is_dumpcap) = match (which("dumpcap"), which("tcpdump")) {
        (Some(d), _) => (d, true),
        (None, Some(t)) => (t, false),
        _ => bail!("нужен dumpcap (wireshark-cli) или tcpdump с правом захвата на lo"),
    };
    let marker = format!("PLAINTEXT-MARKER-{}", uuid::Uuid::new_v4().simple());
    let mut run = Run::new("pcap_no_plaintext", a, "star", json!({ "marker": marker }))?;
    let mut rng = StdRng::seed_from_u64(a.seed);
    let mut net = Net::start("pcap", a.nodes, "star").await?;
    let n = net.n();
    let (p_lo, p_hi) = (net.nodes[0].port, net.nodes[n - 1].port);
    let (g_lo, g_hi) = (net.nodes[0].gui, net.nodes[n - 1].gui);
    let overlay = run.dir.join("overlay.pcapng");
    let control = net.dir.join("gui_control.pcapng");   // открытый HTTP панели — в логи, не в результаты
    let cap = |path: &Path, flt: String| {
        let mut c = tokio::process::Command::new(&tool);
        if is_dumpcap { c.args(["-q", "-i", "lo", "-f", &flt, "-w"]).arg(path); }
        else { c.args(["-i", "lo", "-U", "-w"]).arg(path).arg(&flt); }
        c.stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).kill_on_drop(true).spawn()
    };
    let mut caps = vec![cap(&overlay, format!("tcp portrange {p_lo}-{p_hi}"))?, cap(&control, format!("tcp portrange {g_lo}-{g_hi}"))?];
    sleep_s(2.0).await;
    net.converge(60.0, 3).await;
    let mut rows = vec![];
    for k in 0..5 {
        let from = rng.gen_range(0..n);
        let to = other(&mut rng, n, from);
        let r = send(&net, from, to, &format!("{marker} #{k}")).await;
        rows.push(vec![s("message"), s(k), s(from), s(to), s(is_true(&r["delivered"]))]);
        let st = net.api(from, "POST", "/api/store", &[("key", &format!("pcap-{k}")), ("value", &format!("{marker} dht #{k}"))]).await
            .unwrap_or_else(|_| json!({}));
        rows.push(vec![s("dht_store"), s(k), s(from), String::new(), s(is_true(&st["ok"]))]);
    }
    sleep_s(1.0).await;
    for c in &mut caps {
        if let Some(pid) = c.id() { let _ = std::process::Command::new("kill").args(["-INT", &pid.to_string()]).status(); }
        if tokio::time::timeout(Duration::from_secs(5), c.wait()).await.is_err() { let _ = c.start_kill(); }
    }
    close(&mut net, &run.dir.join("node_logs")).await?;
    let data = std::fs::read(&overlay).unwrap_or_default();
    let ctrl = std::fs::read(&control).unwrap_or_default();
    let m = marker.as_bytes();
    let contains = |b: &[u8]| b.windows(m.len()).any(|w| w == m);
    let (in_overlay, in_ctrl) = (contains(&data), contains(&ctrl));
    if which("tshark").is_some() {
        let phs = std::process::Command::new("tshark").arg("-r").arg(&overlay).args(["-q", "-z", "io,phs"]).output()?;
        let sh = std::process::Command::new("tshark").arg("-r").arg(&overlay)
            .args(["-Y", "tls.handshake.type == 2", "-T", "fields", "-e", "tls.handshake.extensions.supported_version"]).output()?;
        let vers: Vec<String> = String::from_utf8_lossy(&sh.stdout).split_whitespace().map(str::to_owned).collect();
        let mut uniq = vers.clone();
        uniq.sort();
        uniq.dedup();
        let names = |v: &str| match v { "0x0304" => "TLS 1.3", "0x0303" => "TLS 1.2", _ => "?" };
        let line = uniq.iter().map(|v| format!("{} ({v}) × {}", names(v), vers.iter().filter(|x| *x == v).count())).collect::<Vec<_>>().join(", ");
        std::fs::write(run.dir.join("tshark_protocols.txt"),
            format!("{}\nСогласованная версия в ServerHello: {line}\n", String::from_utf8_lossy(&phs.stdout)))?;
    }
    rows.push(vec![s("check_overlay_contains_marker"), String::new(), String::new(), String::new(), s(in_overlay)]);
    rows.push(vec![s("control_gui_contains_marker"), String::new(), String::new(), String::new(), s(in_ctrl)]);
    run.finish(&["what", "k", "sender", "receiver", "result"], &rows,
        &format!("Захват `{}` на lo, порты оверлея {p_lo}–{p_hi}, во время 5 сообщений и 5 DHT STORE, содержащих маркер `{marker}`.\n\n\
                  - маркер в трафике оверлея (overlay.pcapng, {} байт): **{}**\n\
                  - положительный контроль: тот же маркер в HTTP-запросах к панели узла (порты {g_lo}–{g_hi}, открытый localhost-HTTP): {}\n\
                  - протоколы в захвате оверлея — tshark_protocols.txt",
                 if is_dumpcap { "dumpcap" } else { "tcpdump" }, data.len(),
                 if in_overlay { "НАЙДЕН — утечка!" } else { "не найден" },
                 if in_ctrl { "найден — метод поиска работает" } else { "НЕ найден — проверка недостоверна" }))?;
    Ok(!in_overlay && in_ctrl)
}

// ── channel: TLS 1.3 против собственного рукопожатия (ТЗ §11.2 / §11.3) ──────

async fn schannel(a: &Args) -> Result<bool> {
    let pairs = (a.n / 2).max(5);
    let mut run = Run::new("channel_compare", a, "star", json!({ "channels": ["tls", "ake"], "pings": a.n, "lookups": a.n, "pairs": pairs, "messages_per_pair": 3 }))?;
    let mut rng = StdRng::seed_from_u64(a.seed);
    let mut rows = vec![];
    for ch in ["tls", "ake"] {
        let mut net = Net::start_with(&format!("ch_{ch}"), a.nodes, "star", |_| vec!["--channel".into(), ch.into()]).await?;
        net.converge(60.0, 3).await;
        let n = net.n();
        // PING по адресу: новое соединение (рукопожатие канала) + один RPC
        for k in 0..a.n {
            let i = rng.gen_range(0..n);
            let j = other(&mut rng, n, i);
            let addr = net.addr(j);
            let r = net.api(i, "POST", "/api/ping", &[("addr", &addr)]).await.unwrap_or_else(|_| json!({}));
            rows.push(vec![s(ch), s("ping"), s(k), s(i), s(j), jv(&r["rtt_ms"])]);
        }
        for k in 0..a.n {
            let i = rng.gen_range(0..n);
            let t = other(&mut rng, n, i);
            let r = lookup(&net, i, &net.id(t)).await;
            if is_true(&r["found_target"]) { rows.push(vec![s(ch), s("lookup"), s(k), s(i), s(t), jv(&r["duration_ms"])]); }
        }
        let mut ps: Vec<(usize, usize)> = (0..n).flat_map(|x| (x + 1..n).map(move |y| (x, y))).collect();
        ps.shuffle(&mut rng);
        for (p, &(x, y)) in ps.iter().take(pairs).enumerate() {
            for m in 0..3 {
                let r = send(&net, x, y, &format!("ch-{ch}-{p}-{m}")).await;
                if !is_true(&r["delivered"]) { continue; }
                let kind = if r["tunnel_reused"].as_bool() == Some(false) { "first" } else { "session" };
                rows.push(vec![s(ch), s(kind), s(p), s(x), s(y), jv(&r["rtt_ms"])]);
                if kind == "first" { rows.push(vec![s(ch), s("tunnel_build"), s(p), s(x), s(y), jv(&r["tunnel_build_ms"])]); }
            }
        }
        close(&mut net, &run.dir.join("node_logs").join(ch)).await?;
    }
    let col = |ch: &str, m: &str| -> Vec<f64> { rows.iter().filter(|r| r[0] == ch && r[1] == m).filter_map(|r| r[5].parse().ok()).collect() };
    let mut lines = vec![];
    for (m, title) in [("ping", "PING по новому соединению (рукопожатие + 1 RPC), мс"), ("lookup", "lookup, мс"),
                       ("tunnel_build", "построение туннеля, мс"), ("first", "первое сообщение, RTT до APP_ACK, мс"),
                       ("session", "установленная сессия, RTT до APP_ACK, мс")] {
        lines.push(format!("- {title}\n  - TLS 1.3: {}\n  - AKE: {}", fmt(&col("tls", m)), fmt(&col("ake", m))));
    }
    run.finish(&["channel", "metric", "n", "from", "to", "value_ms"], &rows,
        &format!("Одна и та же сеть из {} узлов поднимается дважды: с каналом TLS 1.3 и с собственным рукопожатием (AKE, §11.3). \
                  Остальное одинаково (seed, пары, сборка `{}`).\n\n{}",
                 a.nodes, if cfg!(debug_assertions) { "debug" } else { "release" }, lines.join("\n")))?;
    Ok(true)
}

// ── media: может ли туннель нести голосовой поток (план звонков, шаг 0) ─────────
//
// Поток как у голоса: пакет 80 байт каждые 20 мс (Opus 20 мс ≈ 24 кбит/с), пакеты уходят по расписанию,
// не дожидаясь подтверждения. Узлы стенда на одной машине — часы общие, задержка в одну сторону меряется точно.
// Режимы — способы соединений между узлами (`MEDIA_MODES`); сравнение — главный результат шага.

/// (название режима, дополнительные флаги узла)
const MEDIA_MODES: &[(&str, &[&str])] = &[("per_request", &["--no-reuse-connections"]), ("pool", &[]),
    ("pool_all", &["--pool-scope", "all"])];

const MEDIA_PACKETS: u32 = 250;     // 5 с потока
const MEDIA_INTERVAL_MS: u64 = 20;
const MEDIA_SIZE: usize = 80;

struct StreamStat { received: usize, one_way: Vec<f64>, jitter_ms: f64, reordered: usize }

/// Задержка в одну сторону, джиттер (RFC 3550, §6.4.1) и переупорядочивание по принятым пакетам.
fn stream_stat(pkts: &[Value]) -> StreamStat {
    let mut arr: Vec<(u64, u64, u64)> = pkts.iter().filter_map(|p| Some((
        p["recv_us"].as_u64()?, p["sent_us"].as_u64()?, p["seq"].as_u64()?))).collect();
    arr.sort_by_key(|x| x.0);   // порядок прихода
    let one_way: Vec<f64> = arr.iter().map(|(r, s, _)| (*r as f64 - *s as f64) / 1000.0).collect();
    let mut j = 0.0;
    for w in arr.windows(2) {
        let d = (w[1].0 as f64 - w[0].0 as f64) - (w[1].1 as f64 - w[0].1 as f64);
        j += (d.abs() / 1000.0 - j) / 16.0;
    }
    let mut max_seq = 0u64;
    let mut reordered = 0;
    for (k, (_, _, seq)) in arr.iter().enumerate() {
        if k > 0 && *seq < max_seq { reordered += 1; }
        max_seq = max_seq.max(*seq);
    }
    StreamStat { received: arr.len(), one_way, jitter_ms: j, reordered }
}

async fn smedia(a: &Args) -> Result<bool> {
    let pairs = a.runs.max(1);
    let mut run = Run::new("media_stream", a, "star", json!({
        "modes": MEDIA_MODES.iter().map(|m| m.0).collect::<Vec<_>>(), "pairs": pairs,
        "packets": MEDIA_PACKETS, "interval_ms": MEDIA_INTERVAL_MS, "size": MEDIA_SIZE,
    }))?;
    let mut rng = StdRng::seed_from_u64(a.seed);
    let mut rows = vec![];      // по пакету
    let mut streams = vec![];   // по потоку
    let mut lines = vec![];
    for (mode, flags) in MEDIA_MODES {
        let flags: Vec<String> = flags.iter().map(|f| f.to_string()).collect();
        let mut net = Net::start_with(&format!("media_{mode}"), a.nodes, "star", |_| flags.clone()).await?;
        net.converge(60.0, 3).await;
        let n = net.n();
        let (mut all_ow, mut jit, mut loss, mut acks) = (vec![], vec![], vec![], vec![]);
        for p in 0..pairs {
            let x = rng.gen_range(0..n);
            let y = other(&mut rng, n, x);
            let sid = format!("{mode}-{p}");
            let send = super::net::api(net.gui(x), "POST", "/api/probe", &[
                ("target", &net.id(y)), ("stream", &sid), ("count", &s(MEDIA_PACKETS)),
                ("interval_ms", &s(MEDIA_INTERVAL_MS)), ("size", &s(MEDIA_SIZE))], 300).await
                .unwrap_or_else(|e| json!({ "ok": false, "error": e.to_string() }));
            sleep_s(1.0).await;     // догнать хвост потока
            let got = net.api(y, "GET", "/api/probe_results", &[("stream", &sid)]).await.unwrap_or_else(|_| json!({}));
            let pk = got["packets"].as_array().cloned().unwrap_or_default();
            let st = stream_stat(&pk);
            for v in &pk {
                if let (Some(seq), Some(r), Some(sn)) = (v["seq"].as_u64(), v["recv_us"].as_u64(), v["sent_us"].as_u64()) {
                    rows.push(vec![s(mode), s(p), s(x), s(y), s(seq), format!("{:.2}", (r as f64 - sn as f64) / 1000.0)]);
                }
            }
            let rep = &send["report"];
            let ack: Vec<f64> = rep["ack_ms"].as_array().map(|a| a.iter().filter_map(|v| v.as_f64()).collect()).unwrap_or_default();
            let lost = MEDIA_PACKETS as usize - st.received.min(MEDIA_PACKETS as usize);
            let ow = stat(&st.one_way);
            streams.push(vec![s(mode), s(p), s(x), s(y), s(st.received), s(lost), f1(ow.median), ow.p95.map(f1).unwrap_or_default(),
                f1(ow.max), format!("{:.2}", st.jitter_ms), s(st.reordered), jv(&rep["warmup_ms"]), f1(stat(&ack).median),
                jv(&send["error"])]);
            all_ow.extend(st.one_way.iter().copied());
            jit.push(st.jitter_ms);
            loss.push(100.0 * lost as f64 / MEDIA_PACKETS as f64);
            acks.extend(ack);
        }
        close(&mut net, &run.dir.join("node_logs").join(mode)).await?;
        let ow = stat(&all_ow);
        let verdict = match ow.p95.unwrap_or(ow.max) {
            x if x <= 150.0 => "укладывается в 150 мс (хорошо по ITU-T G.114)",
            x if x <= 300.0 => "150–300 мс (терпимо)",
            _ => "больше 300 мс — для голоса непригодно",
        };
        lines.push(format!("**{mode}**
- задержка в одну сторону, мс: {}
- джиттер (RFC 3550) по потокам, мс: {}
                            - потери по потокам, %: {}
- подтверждение пакета у отправителя (TUNNEL_ACK всей цепочки), мс: {}
                            - вывод по p95: {verdict}", fmt(&all_ow), fmt(&jit), fmt(&loss), fmt(&acks)));
    }
    run.csv("streams.csv", &["mode", "pair", "from", "to", "received", "lost", "one_way_median_ms", "one_way_p95_ms",
                             "one_way_max_ms", "jitter_ms", "reordered", "warmup_ms", "ack_median_ms", "error"], &streams)?;
    run.finish(&["mode", "pair", "from", "to", "seq", "one_way_ms"], &rows,
        &format!("Поток как у голоса: {MEDIA_PACKETS} пакетов по {MEDIA_SIZE} байт каждые {MEDIA_INTERVAL_MS} мс,                   {pairs} случайных пар, сеть {} узлов (звезда), сборка `{}`. Задержка — от отправки до прихода к получателю                   (общие часы стенда), включая ожидание в очереди туннеля. По потокам — `streams.csv`.

{}",
                 a.nodes, if cfg!(debug_assertions) { "debug" } else { "release" }, lines.join("

")))?;
    Ok(true)
}

// ── call: звонок — сигнализация, ключ звонка, медиапоток TUNNEL_MEDIA (план звонков, шаги 2–4) ──
//
// Пара узлов: вызов → ответ (время установления: от вызова до активного звонка у звонящего, т.е. до прихода
// подписанного ответа), затем поток как у голоса (250 × 80 байт / 20 мс) по TUNNEL_MEDIA — без подтверждений и
// мимо очереди туннеля; у вызываемого — расшифровка ключом звонка, окно повторов, статистика. Задержка — по общим
// часам стенда. Сравнение с путём `media` (пакеты с подтверждением всей цепочки) — в summary.

async fn scall(a: &Args) -> Result<bool> {
    let pairs = a.runs.max(1);
    let mut run = Run::new("call_stream", a, "star", json!({
        "pairs": pairs, "packets": MEDIA_PACKETS, "interval_ms": MEDIA_INTERVAL_MS, "size": MEDIA_SIZE,
    }))?;
    let mut rng = StdRng::seed_from_u64(a.seed);
    let mut net = Net::start(&format!("call"), a.nodes, "star").await?;
    net.converge(60.0, 3).await;
    let n = net.n();
    let mut rows = vec![];
    let (mut setup, mut ow, mut ow_p95, mut jit, mut loss, mut rej) = (vec![], vec![], vec![], vec![], vec![], 0u64);
    let mut ok = true;
    for p in 0..pairs {
        let x = rng.gen_range(0..n);
        let y = other(&mut rng, n, x);
        let st = net.api(x, "POST", "/api/call/start", &[("target", &net.id(y))]).await.unwrap_or_default();
        let Some(cid) = st["call_id"].as_str().map(str::to_owned) else { ok = false; eprintln!("вызов {p}: {st}"); continue };
        // вызываемый видит входящий → отвечает
        let mut answered = false;
        for _ in 0..100 {
            let v = net.api(y, "GET", "/api/calls", &[]).await.unwrap_or_default();
            if v["calls"].as_array().is_some_and(|l| l.iter().any(|c| c["call_id"] == cid.as_str() && c["state"] == "ringing")) {
                answered = net.api(y, "POST", "/api/call/accept", &[("call_id", &cid)]).await.map(|r| is_true(&r["ok"])).unwrap_or(false);
                break;
            }
            sleep_s(0.05).await;
        }
        let mut c = json!({});
        for _ in 0..100 {
            let v = net.api(x, "GET", "/api/calls", &[]).await.unwrap_or_default();
            c = v["calls"].as_array().and_then(|l| l.iter().find(|c| c["call_id"] == cid.as_str()).cloned()).unwrap_or_default();
            if c["state"] == "active" { break; }
            sleep_s(0.05).await;
        }
        if !answered || c["state"] != "active" { ok = false; eprintln!("звонок {p} не установлен: {c}"); continue; }
        let setup_ms = c["setup_ms"].as_f64().unwrap_or(f64::NAN);
        let m = super::net::api(net.gui(x), "POST", "/api/call/media", &[("call_id", &cid), ("count", &s(MEDIA_PACKETS)),
            ("interval_ms", &s(MEDIA_INTERVAL_MS)), ("size", &s(MEDIA_SIZE))], 300).await.unwrap_or_default();
        sleep_s(1.0).await;
        let v = net.api(y, "GET", "/api/calls", &[]).await.unwrap_or_default();
        let r = v["calls"].as_array().and_then(|l| l.iter().find(|c| c["call_id"] == cid.as_str()).cloned()).unwrap_or_default();
        let _ = net.api(x, "POST", "/api/call/hangup", &[("call_id", &cid)]).await;
        let received = r["rx"]["packets"].as_u64().unwrap_or(0);
        let lost = MEDIA_PACKETS as u64 - received.min(MEDIA_PACKETS as u64);
        rej += r["rx"]["rejected"].as_u64().unwrap_or(0);
        rows.push(vec![s(p), s(x), s(y), f1(setup_ms), jv(&m["report"]["sent"]), s(received), s(lost),
            jv(&r["one_way_ms"]["median"]), jv(&r["one_way_ms"]["p95"]), jv(&r["one_way_ms"]["max"]),
            format!("{:.2}", r["rx"]["jitter_ms"].as_f64().unwrap_or(f64::NAN)), jv(&r["rx"]["reordered"]), jv(&r["rx"]["rejected"])]);
        setup.push(setup_ms);
        if let Some(v) = r["one_way_ms"]["median"].as_f64() { ow.push(v); }
        if let Some(v) = r["one_way_ms"]["p95"].as_f64() { ow_p95.push(v); }
        jit.push(r["rx"]["jitter_ms"].as_f64().unwrap_or(f64::NAN));
        loss.push(100.0 * lost as f64 / MEDIA_PACKETS as f64);
    }
    close(&mut net, &run.dir.join("node_logs")).await?;
    let worst = ow_p95.iter().copied().fold(0.0, f64::max);
    let verdict = if worst <= 150.0 { "укладывается в 150 мс (хорошо по ITU-T G.114)" } else if worst <= 300.0 { "150–300 мс (терпимо)" } else { "больше 300 мс" };
    run.finish(&["pair", "caller", "callee", "setup_ms", "sent", "received", "lost", "one_way_median_ms", "one_way_p95_ms",
                 "one_way_max_ms", "jitter_ms", "reordered", "rejected"], &rows,
        &format!("Звонок: вызов → ответ → поток {MEDIA_PACKETS} × {MEDIA_SIZE} байт / {MEDIA_INTERVAL_MS} мс по TUNNEL_MEDIA \
                  (без подтверждений, мимо очереди туннеля; AES-256-GCM ключом звонка) → завершение. {pairs} пар, сеть {} узлов, \
                  сборка `{}`. Задержка — от отправки до расшифровки у получателя (общие часы стенда).\n\n\
                  - установление звонка (вызов → ответ у звонящего), мс: {}\n\
                  - медиа, задержка в одну сторону — медианы по звонкам, мс: {}\n\
                  - медиа, p95 по звонкам, мс: {}\n\
                  - джиттер (RFC 3550) по звонкам, мс: {}\n\
                  - потери по звонкам, %: {}\n\
                  - отвергнуто окном повторов / проверкой ключа: {rej}\n\
                  - вывод по худшему p95: {verdict}",
                 a.nodes, if cfg!(debug_assertions) { "debug" } else { "release" },
                 fmt(&setup), fmt(&ow), fmt(&ow_p95), fmt(&jit), fmt(&loss)))?;
    Ok(ok)
}

// ── voice: звук звонка целиком — Opus, джиттер-буфер, смешивание; 1:1 и групповой (шаги 5–10 плана) ──

async fn call_session(net: &Net, i: usize, sid: &str) -> Value {
    let v = net.api(i, "GET", "/api/calls", &[]).await.unwrap_or_default();
    v["sessions"].as_array().and_then(|l| l.iter().find(|c| c["id"] == sid).cloned()).unwrap_or_default()
}

async fn wait_call(net: &Net, i: usize, sid: &str, f: impl Fn(&Value) -> bool) -> Option<Value> {
    for _ in 0..200 {
        let c = call_session(net, i, sid).await;
        if f(&c) { return Some(c); }
        sleep_s(0.05).await;
    }
    None
}

async fn tone(net: &Net, i: usize, freq: f64, secs: f64, listen: &str) -> Value {
    super::net::api(net.gui(i), "POST", "/api/call/tone", &[("freq", &s(freq)), ("seconds", &s(secs)), ("listen", listen)], 120)
        .await.map(|v| v["report"].clone()).unwrap_or_default()
}

fn active_legs(c: &Value) -> usize { c["peers"].as_array().map_or(0, |p| p.iter().filter(|x| x["state"] == "active").count()) }

/// Звук звонка: каждый участник вместо микрофона играет свой тон (тестовый источник узла), узел кодирует его
/// Opus и шлёт собеседникам; принятое проходит джиттер-буфер, восстановление потерь и смешивание. Проверяется,
/// что каждый слышит ровно тоны собеседников, и снимается качество: буфер, восстановленное по FEC, замаскированное,
/// задержка «рот — ухо» и MOS по E-модели.
async fn svoice(a: &Args) -> Result<bool> {
    const SECS: f64 = 6.0;
    const TONES: [f64; 3] = [440.0, 554.0, 660.0];
    let calls = a.runs.max(1);
    let mut run = Run::new("voice_call", a, "star", json!({
        "calls_1to1": calls, "calls_group3": calls.div_ceil(2), "tone_seconds": SECS, "codec": "opus/16000/20, 24 кбит/с, FEC",
        "e_model": "ITU-T G.107 упрощённая: Ie=10, Bpl=20 (допущение для Opus)",
    }))?;
    let mut rng = StdRng::seed_from_u64(a.seed);
    let mut net = Net::start("voice", a.nodes, "star").await?;
    net.converge(60.0, 3).await;
    let n = net.n();
    let listen = TONES.iter().map(|f| s(f)).collect::<Vec<_>>().join(",");
    let mut rows = vec![];
    let (mut setup, mut mos, mut m2e, mut lossa, mut buf) = (vec![], vec![], vec![], vec![], vec![]);
    let (mut heard_ok, mut total) = (0, 0);
    let mut ok = true;
    let plan: Vec<usize> = (0..calls).map(|_| 2).chain((0..calls.div_ceil(2)).map(|_| 3)).collect();
    for (k, size) in plan.into_iter().enumerate() {
        // участники: разные узлы
        let mut who = vec![rng.gen_range(0..n)];
        while who.len() < size { let x = rng.gen_range(0..n); if !who.contains(&x) { who.push(x); } }
        let t0 = std::time::Instant::now();
        let st = if size == 2 {
            net.api(who[0], "POST", "/api/call/start", &[("target", &net.id(who[1]))]).await
        } else {
            let m = who[1..].iter().map(|&i| net.id(i)).collect::<Vec<_>>().join(",");
            net.api(who[0], "POST", "/api/call/start", &[("members", &m), ("name", "сценарий")]).await
        }.unwrap_or_default();
        let Some(sid) = st["call_id"].as_str().map(str::to_owned) else { ok = false; eprintln!("вызов {k}: {st}"); continue };
        for &i in &who[1..] {
            if wait_call(&net, i, &sid, |c| c["state"] == "ringing").await.is_some() {
                let _ = net.api(i, "POST", "/api/call/accept", &[("call_id", &sid)]).await;
            }
        }
        let mut up = true;
        for &i in &who { up &= wait_call(&net, i, &sid, |c| active_legs(c) == size - 1).await.is_some(); }
        let setup_ms = t0.elapsed().as_secs_f64() * 1000.0;
        if !up { ok = false; eprintln!("звонок {k} не установлен"); let _ = net.api(who[0], "POST", "/api/call/hangup", &[("call_id", &sid)]).await; continue; }
        let reps = futures::future::join_all(who.iter().enumerate().map(|(j, &i)| tone(&net, i, TONES[j], SECS, &listen))).await;
        total += 1;
        let mut all_heard = true;
        for (j, r) in reps.iter().enumerate() {
            let mut want: Vec<f64> = (0..size).filter(|&x| x != j).map(|x| TONES[x]).collect();
            want.sort_by(|a, b| a.partial_cmp(b).unwrap());
            let got: Vec<f64> = r["heard"].as_array().map(|a| a.iter().filter_map(Value::as_f64).collect()).unwrap_or_default();
            all_heard &= got == want;
        }
        if all_heard { heard_ok += 1; }
        // качество по каждой ноге у каждого участника
        let (mut q_mos, mut q_m2e, mut q_loss, mut q_buf, mut fec, mut conc, mut late, mut played) = (vec![], vec![], vec![], vec![], 0, 0, 0, 0);
        for &i in &who {
            let c = call_session(&net, i, &sid).await;
            for p in c["peers"].as_array().cloned().unwrap_or_default() {
                let q = &p["quality"];
                if let Some(v) = q["mos"].as_f64() { q_mos.push(v); }
                if let Some(v) = q["mouth_to_ear_ms"].as_f64() { q_m2e.push(v); }
                if let Some(v) = q["loss_audible_pct"].as_f64() { q_loss.push(v); }
                if let Some(v) = q["audio"]["buffer_ms"].as_f64() { q_buf.push(v); }
                fec += q["audio"]["fec"].as_u64().unwrap_or(0);
                conc += q["audio"]["concealed"].as_u64().unwrap_or(0);
                late += q["audio"]["late"].as_u64().unwrap_or(0);
                played += q["audio"]["played"].as_u64().unwrap_or(0);
            }
        }
        let _ = net.api(who[0], "POST", "/api/call/hangup", &[("call_id", &sid)]).await;
        for &i in &who[1..] { let _ = net.api(i, "POST", "/api/call/hangup", &[("call_id", &sid)]).await; }
        sleep_s(0.5).await;
        let avg = |v: &[f64]| if v.is_empty() { f64::NAN } else { v.iter().sum::<f64>() / v.len() as f64 };
        rows.push(vec![s(k), if size == 2 { "1:1".into() } else { format!("группа {size}") },
            who.iter().map(|x| x.to_string()).collect::<Vec<_>>().join(" "), f1(setup_ms), s(all_heard),
            s(played), s(fec), s(conc), s(late), f1(avg(&q_buf)), f1(avg(&q_m2e)), format!("{:.2}", avg(&q_loss)), format!("{:.2}", avg(&q_mos))]);
        setup.push(setup_ms); mos.extend(q_mos); m2e.extend(q_m2e); lossa.extend(q_loss); buf.extend(q_buf);
    }
    close(&mut net, &run.dir.join("node_logs")).await?;
    run.finish(&["call", "kind", "nodes", "setup_ms", "heard_ok", "played", "fec", "concealed", "late", "buffer_ms",
                 "mouth_to_ear_ms", "loss_audible_pct", "mos"], &rows,
        &format!("Звук звонка: каждый участник {SECS} с играет свой тон (тестовый источник узла вместо микрофона) → Opus 16 кГц / 20 мс \
                  → ключ звонка → TUNNEL_MEDIA через ≥2 ретранслятора → джиттер-буфер, FEC/маскировка, смешивание у получателя. \
                  Звонков 1:1 — {calls}, групповых (3 участника, «каждый с каждым») — {}; сеть {} узлов, сборка `{}`.\n\n\
                  - каждый слышит ровно тоны собеседников: {heard_ok} из {total}\n\
                  - установление (вызов → все ноги активны у всех), мс: {}\n\
                  - глубина джиттер-буфера по ногам, мс: {}\n\
                  - задержка «рот — ухо» (сеть + буфер + кадр + кодер), мс: {}\n\
                  - слышимые потери (замаскированные кадры), %: {}\n\
                  - MOS по E-модели (Ie=10, Bpl=20 — допущение для Opus): {}",
                 calls.div_ceil(2), a.nodes, if cfg!(debug_assertions) { "debug" } else { "release" },
                 fmt(&setup), fmt(&buf), fmt(&m2e), fmt(&lossa), fmt(&mos)))?;
    Ok(ok && heard_ok == total)
}

// ── files: передача файлов, goodput и доля служебного трафика (§13.2, §15.2) ──

async fn sfiles(a: &Args) -> Result<bool> {
    let sizes: [(&str, usize); 3] = [("100 КиБ", 100 * 1024), ("1 МиБ", 1024 * 1024), ("10 МиБ", 10 * 1024 * 1024)];
    let per = a.runs.max(1);   // по умолчанию 5 (§15.3: ≥ 5 передач каждого размера)
    let mut run = Run::new("files_transfer", a, "star", json!({ "sizes_bytes": sizes.iter().map(|x| x.1).collect::<Vec<_>>(), "transfers_per_size": per }))?;
    let mut rng = StdRng::seed_from_u64(a.seed);
    let mut net = Net::start("files", a.nodes, "star").await?;
    net.converge(60.0, 3).await;
    let n = net.n();
    let fdir = net.dir.join("files");
    std::fs::create_dir_all(&fdir)?;
    let mut rows = vec![];
    for (label, size) in sizes {
        for k in 0..per {
            let from = rng.gen_range(0..n);
            let to = other(&mut rng, n, from);
            let name = format!("f{}_{k}.bin", size / 1024);
            let path = fdir.join(&name);
            let mut data = vec![0u8; size];
            rng.fill(&mut data[..]);
            std::fs::write(&path, &data)?;
            let abs = std::fs::canonicalize(&path)?.display().to_string();
            let id = net.id(to);
            let t0 = Instant::now();
            let r = super::net::api(net.nodes[from].gui, "POST", "/api/send_file", &[("target", &id), ("path", &abs)], 900).await
                .unwrap_or_else(|e| json!({ "ok": false, "error": e.to_string() }));
            let wall = ms_since(t0);
            let rep = &r["report"];
            // целостность у получателя: файл в его downloads/ и SHA-256 совпадает
            let got = std::fs::read(net.dir.join(format!("state{to}/downloads/{name}"))).ok();
            let verified = got.map(|g| Sha256::digest(&g) == Sha256::digest(&data)).unwrap_or(false);
            rows.push(vec![s(label), s(size), s(k), s(from), s(to), s(is_true(&r["ok"])), jv(&rep["duration_ms"]), f1(wall),
                           jv(&rep["goodput_mib_s"]).chars().take(8).collect(), jv(&rep["bytes_out"]), jv(&rep["bytes_in"]),
                           jv(&rep["overhead_share"]).chars().take(8).collect(), jv(&rep["retransmits"]), s(verified), jv(&r["error"])]);
            let _ = std::fs::remove_file(&path);
        }
    }
    close(&mut net, &run.dir.join("node_logs")).await?;
    let mut lines = vec![];
    for (label, _) in sizes {
        let sub: Vec<&Vec<String>> = rows.iter().filter(|r| r[0] == label).collect();
        let ok: Vec<&&Vec<String>> = sub.iter().filter(|r| r[5] == "true" && r[13] == "true").collect();
        let gp: Vec<f64> = ok.iter().filter_map(|r| r[8].parse().ok()).collect();
        let oh: Vec<f64> = ok.iter().filter_map(|r| r[11].parse::<f64>().ok().map(|x| x * 100.0)).collect();
        let rt: u32 = ok.iter().filter_map(|r| r[12].parse::<u32>().ok()).sum();
        lines.push(format!("- {label}: доставлено и SHA-256 совпал {}/{}; goodput, МиБ/с: {}; служебная доля, %: {}; повторов блоков: {rt}",
                           ok.len(), sub.len(), fmt(&gp), fmt(&oh)));
    }
    let all_ok = rows.iter().all(|r| r[5] == "true" && r[13] == "true");
    run.finish(&["size_label", "size_bytes", "n", "sender", "receiver", "ok", "duration_ms", "wall_ms", "goodput_mib_s",
                 "bytes_out", "bytes_in", "overhead_share", "retransmits", "sha256_verified", "error"], &rows,
        &format!("Файлы со случайным содержимым передаются между случайными парами узлов через туннель (2 relay) по протоколу §13.2: \
                  предложение → блоки по {} КиБ окнами по {} → подтверждения → итоговая проверка SHA-256 получателем. \
                  goodput = размер / время от предложения до итогового подтверждения; служебная доля = (байты конвертов в обе стороны − \
                  размер) / байты конвертов — накладные расходы прикладного протокола (подпись, конверт, метаданные, подтверждения), \
                  без TLS/TCP и без умножения на число hop. Целостность проверена по SHA-256 файла в `downloads/` получателя.\n\n{}",
                 40, 8, lines.join("\n")))?;
    Ok(all_ok)
}

// ── failover: отказ ретранслятора основного туннеля — переключение на резервный (§12, §15.2) ──

async fn primary_relays(net: &Net, from: usize, to: usize) -> (Vec<String>, usize) {
    let id = net.id(to);
    match net.api(from, "GET", "/api/route", &[("target", &id)]).await {
        Ok(v) => (v["tunnel"]["hops"].as_array().into_iter().flatten().filter_map(|h| h["addr"].as_str().map(str::to_owned)).collect(),
                  v["tunnel"]["pool"].as_array().map(|p| p.len()).unwrap_or(0)),
        Err(_) => (vec![], 0),
    }
}

async fn sfailover(a: &Args) -> Result<bool> {
    let cfg = super::report::read_config()?;
    let pool = cfg["tunnel"]["pool_size"].as_u64().unwrap_or(1);
    let mut run = Run::new("failover", a, "star", json!({ "injections": a.inj, "pool_size": pool }))?;
    let mut rng = StdRng::seed_from_u64(a.seed);
    let mut net = Net::start("failover", a.nodes, "star").await?;
    net.converge(60.0, 3).await;
    let mut rows = vec![];
    for k in 0..a.inj {
        let alive = net.alive();
        let pair: Vec<usize> = alive.choose_multiple(&mut rng, 2).copied().collect();
        let (from, to) = (pair[0], pair[1]);
        let warm = send(&net, from, to, &format!("fo-warm-{k}")).await;
        sleep_s(1.5).await;   // пул достраивается в фоне
        let (hops, pool_len) = primary_relays(&net, from, to).await;
        let relays: Vec<usize> = hops.iter().filter_map(|h| (0..net.n()).find(|&i| net.addr(i) == *h)).collect();
        if !is_true(&warm["delivered"]) || relays.is_empty() {
            rows.push(vec![s(k), s(from), s(to), String::new(), String::new(), s(pool_len), s(false), String::new(), String::new(),
                           String::new(), String::new(), String::new(), String::new(), s("no-active-tunnel")]);
            continue;
        }
        let pos = k % relays.len();
        let victim = relays[pos];
        net.kill(victim).await;
        let t0 = Instant::now();
        let (mut fails, mut res) = (0, json!({}));
        while t0.elapsed().as_secs_f64() < 60.0 {
            res = send(&net, from, to, &format!("fo-after-{k}-{fails}")).await;
            if is_true(&res["delivered"]) { break; }
            fails += 1;
        }
        let ok = is_true(&res["delivered"]);
        let how = if !ok { "not-recovered" } else if is_true(&res["failover"]) { "failover" }
                  else if is_true(&res["rebuilt_after_failure"]) { "rebuild" } else { "unaffected" };
        rows.push(vec![s(k), s(from), s(to), s(victim), s(pos), s(pool_len), s(ok), s(fails),
                       if ok { f1(ms_since(t0)) } else { String::new() }, jv(&res["detect_ms"]), jv(&res["switch_ms"]),
                       jv(&res["tunnel_build_ms"]), jv(&res["rtt_ms"]), s(how)]);
        net.restart(victim).await?;
        net.converge(20.0, 2).await;
    }
    close(&mut net, &run.dir.join("node_logs")).await?;
    let c = |i: usize, how: &str| -> Vec<f64> { rows.iter().filter(|r| r[13] == how).filter_map(|r| r[i].parse().ok()).collect() };
    let cnt = |how: &str| rows.iter().filter(|r| r[13] == how).count();
    let inj = rows.iter().filter(|r| !r[3].is_empty()).count();
    let rec: Vec<f64> = rows.iter().filter(|r| r[6] == "true").filter_map(|r| r[8].parse().ok()).collect();
    run.finish(&["n", "sender", "receiver", "killed_relay", "hop_index", "pool_len", "delivered", "failed_attempts", "recovery_ms",
                 "detect_ms", "switch_ms", "rebuild_build_ms", "rtt_ms", "outcome"], &rows,
        &format!("Как в 07, но с пулом туннелей: после первой отправки пул достраивается в фоне (1,5 с), затем отключается relay \
                  **основного** туннеля, и сообщение отправляется снова. pool_size = {pool}. detect_ms — от начала отправки до \
                  обнаружения отказа (нет TUNNEL_ACK / ERROR), switch_ms — от обнаружения до TUNNEL_ACK по резервному туннелю.\n\n\
                  - инъекций {inj}: переключение на резервный {}, перестроение маршрута {}, отказ не затронул отправку {}, не восстановлено {}\n\
                  - время восстановления (от отключения до APP_ACK), мс: {}\n\
                  - время обнаружения отказа, мс: переключение — {}; перестроение — {}\n\
                  - время переключения на резервный туннель, мс: {}\n\
                  - построение нового туннеля при перестроении, мс: {}",
                 cnt("failover"), cnt("rebuild"), cnt("unaffected"), cnt("not-recovered"), fmt(&rec),
                 fmt(&c(9, "failover")), fmt(&c(9, "rebuild")), fmt(&c(10, "failover")), fmt(&c(11, "rebuild"))))?;
    Ok(cnt("not-recovered") == 0)
}

// ── strategy: случайный выбор ретрансляторов против выбора по рейтингу (§12, продвинутый уровень) ──

async fn sstrategy(a: &Args) -> Result<bool> {
    let strategies = ["closest", "random", "rating"];
    let (senders_n, warm, measure, drop_p) = (3usize, 10usize, (a.n / 3).max(10), 0.5f64);
    let faulty_n = (a.nodes / 4).max(2);
    let mut run = Run::new("relay_strategy", a, "star", json!({
        "strategies": strategies, "faulty_relays": faulty_n, "fault_drop": drop_p,
        "senders": senders_n, "warmup_per_sender": warm, "measured_per_sender": measure }))?;
    let mut rows = vec![];
    for st in strategies {
        // одинаковые неисправные узлы, отправители и пары для всех стратегий
        let mut rng = StdRng::seed_from_u64(a.seed);
        let mut idx: Vec<usize> = (1..a.nodes).collect();
        idx.shuffle(&mut rng);
        let faulty: Vec<usize> = idx[..faulty_n].to_vec();
        let healthy: Vec<usize> = (0..a.nodes).filter(|i| !faulty.contains(i)).collect();
        let senders: Vec<usize> = healthy.choose_multiple(&mut rng, senders_n).copied().collect();
        let fl = faulty.clone();
        let mut net = Net::start_with(&format!("strat_{st}"), a.nodes, "star", move |i| {
            let mut v = vec!["--relay-selection".to_owned(), st.to_owned()];
            if fl.contains(&i) { v.push("--fault-drop".into()); v.push(drop_p.to_string()); }
            v
        }).await?;
        net.converge(60.0, 3).await;
        for (phase, count) in [("warmup", warm), ("measure", measure)] {
            for k in 0..count {
                for &sn in &senders {
                    let to = *healthy.iter().filter(|&&h| h != sn).collect::<Vec<_>>().choose(&mut rng).copied().unwrap();
                    let r = send(&net, sn, to, &format!("st-{st}-{phase}-{k}")).await;
                    let ok = is_true(&r["delivered"]);
                    let clean = ok && !is_true(&r["failover"]) && !is_true(&r["rebuilt_after_failure"]);
                    rows.push(vec![s(st), s(phase), s(k), s(sn), s(to), s(ok), s(clean), jv(&r["rtt_ms"]),
                                   s(is_true(&r["failover"])), s(is_true(&r["rebuilt_after_failure"]))]);
                }
            }
        }
        // как отправители оценили неисправные узлы
        for &sn in &senders {
            if let Ok(v) = net.api(sn, "GET", "/api/relays", &[]).await {
                std::fs::write(run.dir.join(format!("profiles_{st}_sender{sn}.json")), serde_json::to_string_pretty(&v)?)?;
            }
        }
        std::fs::write(run.dir.join(format!("faulty_{st}.json")), serde_json::to_string(&json!({
            "faulty_nodes": faulty, "faulty_ids": faulty.iter().map(|&i| net.id(i)).collect::<Vec<_>>(), "senders": senders }))?)?;
        close(&mut net, &run.dir.join("node_logs").join(st)).await?;
    }
    let mut lines = vec![];
    for st in strategies {
        let m: Vec<&Vec<String>> = rows.iter().filter(|r| r[0] == st && r[1] == "measure").collect();
        let clean = m.iter().filter(|r| r[6] == "true").count();
        let ok = m.iter().filter(|r| r[5] == "true").count();
        let fo = m.iter().filter(|r| r[8] == "true").count();
        let rb = m.iter().filter(|r| r[9] == "true").count();
        let rtt: Vec<f64> = m.iter().filter(|r| r[5] == "true").filter_map(|r| r[7].parse().ok()).collect();
        lines.push(format!("- **{st}**: доставлено {ok}/{}; без отказа на пути {clean}/{} ({:.0}%); переключений на резервный {fo}, \
                            перестроений {rb}; RTT до APP_ACK, мс: {}", m.len(), m.len(), 100.0 * clean as f64 / m.len().max(1) as f64, fmt(&rtt)));
    }
    run.finish(&["strategy", "phase", "n", "sender", "receiver", "delivered", "clean", "rtt_ms", "failover", "rebuilt"], &rows,
        &format!("Одна и та же сеть ({} узлов, пул туннелей, конфиг продвинутого уровня) с {faulty_n} неисправными ретрансляторами: \
                  каждый с вероятностью {drop_p} отвечает ошибкой вместо пересылки (`--fault-drop`). Неисправные узлы, {senders_n} \
                  отправителя и получатели одинаковы для всех стратегий (seed). Разогрев — {warm} сообщений на отправителя (профили \
                  накапливаются), измерение — {measure} на отправителя. «Без отказа на пути» — доставлено первым же туннелем, без \
                  переключения и перестроения. Профили, которые отправители построили, — profiles_*.json.\n\n{}",
                 a.nodes, lines.join("\n")))?;
    Ok(true)
}

// ── root: tc/netem и nftables через `sudo -n` (пользователь заранее выполняет `sudo -v`) ──

fn sudo(args: &[&str]) -> Result<String> {
    let out = std::process::Command::new("sudo").arg("-n").args(args).output()?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_owned();
        if err.contains("password") {
            bail!("нужны права root для `{}`: выполните `sudo -v` и запустите сценарий снова (файлы результатов останутся вашими)", args.join(" "));
        }
        bail!("sudo {}: {err}", args.join(" "));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Снимает qdisc с lo при выходе из сценария (в том числе по ошибке).
struct NetemGuard;
impl Drop for NetemGuard {
    fn drop(&mut self) { let _ = std::process::Command::new("sudo").args(["-n", "tc", "qdisc", "del", "dev", "lo", "root"]).output(); }
}

/// Сценарий 10 (§15.1, продвинутый уровень): управляемая деградация underlay через tc/netem на loopback.
async fn s10_netem(a: &Args) -> Result<bool> {
    let profiles: [(&str, &str); 5] = [("без деградации", ""), ("задержка 10 мс", "delay 10ms"),
        ("задержка 10±5 мс", "delay 10ms 5ms distribution normal"), ("потери 2%", "loss 2%"), ("10 мс + 2%", "delay 10ms loss 2%")];
    sudo(&["tc", "qdisc", "show", "dev", "lo"])?;   // проверка прав до запуска сети
    let _guard = NetemGuard;
    let mut run = Run::new("10_netem", a, "star", json!({ "profiles": profiles.iter().map(|p| json!({"name": p.0, "netem": p.1})).collect::<Vec<_>>(),
                                                         "lookups_per_profile": a.n, "pairs_per_profile": (a.n / 3).max(5) }))?;
    let mut rng = StdRng::seed_from_u64(a.seed);
    let mut net = Net::start("netem", a.nodes, "star").await?;
    net.converge(60.0, 3).await;
    let n = net.n();
    let mut rows = vec![];
    for (name, spec) in profiles {
        let _ = sudo(&["tc", "qdisc", "del", "dev", "lo", "root"]);
        if !spec.is_empty() {
            let mut args = vec!["tc", "qdisc", "add", "dev", "lo", "root", "netem"];
            args.extend(spec.split_whitespace());
            sudo(&args)?;
        }
        sleep_s(1.0).await;
        for k in 0..a.n {
            let i = rng.gen_range(0..n);
            let t = other(&mut rng, n, i);
            let r = lookup(&net, i, &net.id(t)).await;
            rows.push(vec![s(name), s("lookup"), s(k), s(i), s(t), s(is_true(&r["ok"]) && is_true(&r["found_target"])), jv(&r["duration_ms"])]);
        }
        let mut ps: Vec<(usize, usize)> = (0..n).flat_map(|x| (x + 1..n).map(move |y| (x, y))).collect();
        ps.shuffle(&mut rng);
        for (p, &(x, y)) in ps.iter().take((a.n / 3).max(5)).enumerate() {
            for m in 0..3 {
                let r = send(&net, x, y, &format!("netem-{p}-{m}")).await;
                let kind = if m == 0 { "message_first" } else { "message_session" };
                rows.push(vec![s(name), s(kind), s(p), s(x), s(y), s(is_true(&r["delivered"])), jv(&r["rtt_ms"])]);
            }
        }
    }
    drop(_guard);
    close(&mut net, &run.dir.join("node_logs")).await?;
    let mut lines = vec![];
    for (name, spec) in profiles {
        let pick = |kind: &str| -> (usize, usize, Vec<f64>) {
            let sub: Vec<&Vec<String>> = rows.iter().filter(|r| r[0] == name && r[1] == kind).collect();
            let ok: Vec<f64> = sub.iter().filter(|r| r[5] == "true").filter_map(|r| r[6].parse().ok()).collect();
            (ok.len(), sub.len(), ok)
        };
        let (lo, lt, ld) = pick("lookup");
        let (so, st, sd) = pick("message_session");
        let (fo, ft, _) = pick("message_first");
        lines.push(format!("- **{name}** (`{}`): lookup {lo}/{lt}, мс {}; первое сообщение доставлено {fo}/{ft}; сессия {so}/{st}, RTT мс {}",
                           if spec.is_empty() { "—" } else { spec }, fmt(&ld), fmt(&sd)));
    }
    run.finish(&["profile", "metric", "n", "from", "to", "ok", "value_ms"], &rows,
        &format!("На интерфейс loopback по очереди накладываются профили `tc qdisc … netem` (все узлы на одной машине — деградация \
                  действует на каждое соединение между ними). Сеть одна; на каждом профиле — {} lookup и сообщения. Учтите: на \
                  каждый hop туннеля и каждое рукопожатие задержка приходится отдельно.\n\n{}", a.n, lines.join("\n")))?;
    Ok(true)
}

/// Снимает таблицу nftables при выходе.
struct NftGuard;
impl Drop for NftGuard {
    fn drop(&mut self) { let _ = std::process::Command::new("sudo").args(["-n", "nft", "delete", "table", "inet", "p2p_underlay"]).output(); }
}

/// Ограниченная IP-достижимость (§14, продвинутый уровень): группы A (127.0.1.0/24) и B (127.0.2.0/24) не видят друг
/// друга на уровне IP (nftables); шлюзы (127.0.3.0/24) видят всех. Проверяем, что оверлей связывает A и B через шлюзы.
async fn sunderlay(a: &Args) -> Result<bool> {
    let dry = std::env::var("P2P_UNDERLAY_DRY").is_ok();   // только для проверки механики без root
    let n = a.nodes;
    let gw = 3usize.min(n / 4).max(2);
    let group = |i: usize| -> char { if i < gw { 'G' } else if i % 2 == 0 { 'A' } else { 'B' } };
    let host = move |i: usize| -> String {
        match group(i) { 'G' => format!("127.0.3.{}", i + 1), 'A' => format!("127.0.1.{}", i + 1), _ => format!("127.0.2.{}", i + 1) }
    };
    let rules = "table inet p2p_underlay {\n  chain out {\n    type filter hook output priority 0; policy accept;\n    \
                 ip saddr 127.0.1.0/24 ip daddr 127.0.2.0/24 meta l4proto tcp reject with tcp reset\n    ip saddr 127.0.2.0/24 ip daddr 127.0.1.0/24 meta l4proto tcp reject with tcp reset\n  }\n}\n";
    let mut run = Run::new("underlay", a, "multiseed", json!({ "gateways": gw, "nft_rules": rules, "dry_run": dry,
                                                               "groups": (0..n).map(|i| group(i).to_string()).collect::<Vec<_>>() }))?;
    std::fs::write(run.dir.join("nft_rules.nft"), rules)?;
    let _guard = NftGuard;
    if !dry {
        let f = run.dir.join("nft_rules.nft");
        sudo(&["nft", "-f", &f.display().to_string()])?;
    }
    let mut rng = StdRng::seed_from_u64(a.seed);
    let mut net = Net::start_hosts("underlay", n, "multiseed", host, |_| vec![]).await?;
    net.converge(60.0, 3).await;
    let a_nodes: Vec<usize> = (0..n).filter(|&i| group(i) == 'A').collect();
    let b_nodes: Vec<usize> = (0..n).filter(|&i| group(i) == 'B').collect();
    let mut rows = vec![];
    // 1) прямая достижимость A → B на уровне IP (ожидается: нет)
    for k in 0..10.min(a.n) {
        let (x, y) = (*a_nodes.choose(&mut rng).unwrap(), *b_nodes.choose(&mut rng).unwrap());
        let r = net.api(x, "POST", "/api/ping", &[("addr", &net.addr(y))]).await.unwrap_or_else(|_| json!({}));
        rows.push(vec![s("direct_ping_A_to_B"), s(k), s(x), s(y), s(is_true(&r["ok"])), jv(&r["rtt_ms"])]);
    }
    // 2) lookup B-узла с A-узла (оверлей знает адрес через шлюзы)
    for k in 0..a.n {
        let (x, y) = (*a_nodes.choose(&mut rng).unwrap(), *b_nodes.choose(&mut rng).unwrap());
        let r = lookup(&net, x, &net.id(y)).await;
        rows.push(vec![s("lookup_A_to_B"), s(k), s(x), s(y), s(is_true(&r["found_target"])), jv(&r["duration_ms"])]);
    }
    // 3) сообщение A → B через туннель
    for k in 0..(a.n / 2).max(10) {
        let (x, y) = (*a_nodes.choose(&mut rng).unwrap(), *b_nodes.choose(&mut rng).unwrap());
        let r = send(&net, x, y, &format!("ul-{k}")).await;
        let path = route_of(&net, x, y).await.join(" → ");
        rows.push(vec![s("message_A_to_B"), s(k), s(x), s(y), s(is_true(&r["delivered"])), jv(&r["rtt_ms"]), path]);
    }
    drop(_guard);
    close(&mut net, &run.dir.join("node_logs")).await?;
    let stat_of = |m: &str| -> (usize, usize, Vec<f64>) {
        let sub: Vec<&Vec<String>> = rows.iter().filter(|r| r[0] == m).collect();
        let ok: Vec<f64> = sub.iter().filter(|r| r[4] == "true").filter_map(|r| r[5].parse().ok()).collect();
        (sub.iter().filter(|r| r[4] == "true").count(), sub.len(), ok)
    };
    let (p_ok, p_n, _) = stat_of("direct_ping_A_to_B");
    let (l_ok, l_n, l_ms) = stat_of("lookup_A_to_B");
    let (m_ok, m_n, m_ms) = stat_of("message_A_to_B");
    for r in rows.iter_mut() { while r.len() < 7 { r.push(String::new()); } }
    run.finish(&["check", "n", "from", "to", "ok", "value_ms", "route"], &rows,
        &format!("{n} узлов на разных loopback-адресах: группа A — 127.0.1.0/24, B — 127.0.2.0/24, шлюзы — 127.0.3.0/24 ({gw} узла, \
                  они же bootstrap, схема multi-seed). nftables запрещает IP-трафик между A и B (nft_rules.nft){}. Узлы привязывают \
                  исходящие соединения к своему адресу (`--host`).\n\n\
                  - прямой PING A → B: успешно {p_ok}/{p_n} (ожидается 0 — проверка, что ограничение действует)\n\
                  - lookup B-узла с A-узла: найден {l_ok}/{l_n}; мс {}\n\
                  - сообщение A → B через туннель: доставлено {m_ok}/{m_n}; RTT мс {}; маршруты — столбец route",
                 if dry { " — **ПРОБНЫЙ ПРОГОН БЕЗ ПРАВИЛ (P2P_UNDERLAY_DRY)**" } else { "" }, fmt(&l_ms), fmt(&m_ms)))?;
    Ok(dry || p_ok == 0)
}
