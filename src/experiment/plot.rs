//! experiment/plot.rs — графики из сохранённых CSV (ТЗ §15.3) в SVG, без внешних библиотек:
//! «ящики с усами» поверх отдельных наблюдений и столбцы. Для приёмки (`accept_*`) ещё и stats.md.
//!
//! `p2p-orchestrator plot <каталог>` перестраивает графики любого прогона из его файлов.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use anyhow::Result;

use super::report::stat;

type Row = HashMap<String, String>;

fn read_csv(path: &Path) -> Result<Vec<Row>> {
    if !path.exists() { return Ok(vec![]); }
    let mut rdr = csv::Reader::from_path(path)?;
    let headers = rdr.headers()?.clone();
    let mut out = vec![];
    for rec in rdr.records() {
        let rec = rec?;
        out.push(headers.iter().zip(rec.iter()).map(|(h, v)| (h.to_owned(), v.to_owned())).collect());
    }
    Ok(out)
}

fn num(r: &Row, k: &str) -> Option<f64> { r.get(k)?.parse::<f64>().ok().filter(|x| x.is_finite()) }

/// Булево поле: `true`/`True` (CSV Rust- и Python-драйвера).
fn yes(r: &Row, k: &str) -> bool { r.get(k).is_some_and(|v| v.eq_ignore_ascii_case("true")) }

fn eq(r: &Row, k: &str, v: &str) -> bool { r.get(k).is_some_and(|x| x == v) }

fn col(rows: &[Row], k: &str, keep: impl Fn(&Row) -> bool) -> Vec<f64> {
    rows.iter().filter(|r| keep(r)).filter_map(|r| num(r, k)).collect()
}

// ── SVG ──────────────────────────────────────────────────────────────────────

const W: f64 = 760.0;
const H: f64 = 430.0;
const ML: f64 = 76.0;
const MR: f64 = 28.0;
const MT: f64 = 70.0;
const MB: f64 = 84.0;
const COLORS: [&str; 6] = ["#4c78a8", "#f58518", "#54a24b", "#e45756", "#b279a2", "#72b7b2"];
const INK: &str = "#222";
const MUTED: &str = "#666";
const GRID: &str = "#e3e3e3";

fn esc(s: &str) -> String { s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;") }

struct Svg(String);

impl Svg {
    fn new(title: &str, subtitle: &str) -> Self {
        let mut s = Svg(format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{W}\" height=\"{H}\" viewBox=\"0 0 {W} {H}\" \
             font-family=\"DejaVu Sans, Arial, Helvetica, sans-serif\">\n<rect width=\"100%\" height=\"100%\" fill=\"#fff\"/>\n"));
        s.text(ML, 30.0, title, 17.0, "start", INK, "bold");
        s.text(ML, 50.0, subtitle, 11.5, "start", MUTED, "normal");
        s
    }
    fn text(&mut self, x: f64, y: f64, t: &str, size: f64, anchor: &str, fill: &str, weight: &str) {
        self.0 += &format!("<text x=\"{x:.1}\" y=\"{y:.1}\" font-size=\"{size}\" text-anchor=\"{anchor}\" fill=\"{fill}\" font-weight=\"{weight}\">{}</text>\n", esc(t));
    }
    fn line(&mut self, x1: f64, y1: f64, x2: f64, y2: f64, stroke: &str, width: f64, dash: bool) {
        let d = if dash { " stroke-dasharray=\"6 4\"" } else { "" };
        self.0 += &format!("<line x1=\"{x1:.1}\" y1=\"{y1:.1}\" x2=\"{x2:.1}\" y2=\"{y2:.1}\" stroke=\"{stroke}\" stroke-width=\"{width}\"{d}/>\n");
    }
    fn rect(&mut self, x: f64, y: f64, w: f64, h: f64, fill: &str, opacity: f64, stroke: &str) {
        self.0 += &format!("<rect x=\"{x:.1}\" y=\"{y:.1}\" width=\"{w:.1}\" height=\"{:.1}\" fill=\"{fill}\" fill-opacity=\"{opacity}\" stroke=\"{stroke}\" stroke-width=\"1.4\"/>\n", h.max(0.5));
    }
    fn circle(&mut self, cx: f64, cy: f64, r: f64, fill: &str, opacity: f64) {
        self.0 += &format!("<circle cx=\"{cx:.1}\" cy=\"{cy:.1}\" r=\"{r}\" fill=\"{fill}\" fill-opacity=\"{opacity}\"/>\n");
    }
    fn diamond(&mut self, cx: f64, cy: f64, r: f64) {
        self.0 += &format!("<polygon points=\"{:.1},{cy:.1} {cx:.1},{:.1} {:.1},{cy:.1} {cx:.1},{:.1}\" fill=\"#fff\" stroke=\"{INK}\" stroke-width=\"1.3\"/>\n",
                           cx - r, cy - r, cx + r, cy + r);
    }
    /// Подпись категории под осью: несколько строк + строка n=… серым.
    fn category(&mut self, cx: f64, label: &str, n: Option<usize>) {
        let mut y = H - MB + 20.0;
        for l in label.split('\n') { self.text(cx, y, l, 12.0, "middle", INK, "normal"); y += 15.0; }
        if let Some(n) = n { self.text(cx, y, &format!("n={n}"), 11.0, "middle", MUTED, "normal"); }
    }
    fn end(mut self, source: &str) -> String {
        self.text(W - MR, H - 10.0, &format!("данные: {source}"), 10.0, "end", "#999", "normal");
        self.0 += "</svg>\n";
        self.0
    }
}

struct Axis { lo: f64, hi: f64, step: f64 }

impl Axis {
    /// Ось от нуля (для неотрицательных величин) до «круглого» значения выше максимума.
    fn new(min: f64, max: f64) -> Self {
        let lo = if min >= 0.0 { 0.0 } else { min };
        let hi = if max > lo { max } else { lo + 1.0 };
        let raw = (hi - lo) / 5.0;
        let mag = 10f64.powf(raw.log10().floor());
        let norm = raw / mag;
        let step = mag * if norm <= 1.0 { 1.0 } else if norm <= 2.0 { 2.0 } else if norm <= 2.5 { 2.5 } else if norm <= 5.0 { 5.0 } else { 10.0 };
        let (lo, hi) = ((lo / step).floor() * step, (hi / step).ceil() * step);
        Self { lo, hi: if hi > lo { hi } else { lo + step }, step }
    }
    fn y(&self, v: f64) -> f64 { MT + (H - MT - MB) * (1.0 - (v - self.lo) / (self.hi - self.lo)) }
    /// Подпись деления с числом знаков, достаточным для шага (шаг 2.5 → 7.5, а не 8).
    fn label(&self, v: f64) -> String {
        let dec = (0..3).find(|d| { let m = 10f64.powi(*d); ((self.step * m) - (self.step * m).round()).abs() < 1e-6 }).unwrap_or(3);
        format!("{v:.*}", dec as usize)
    }
    fn draw(&self, svg: &mut Svg, ylabel: &str) {
        let mut v = self.lo;
        while v <= self.hi + self.step * 1e-6 {
            let y = self.y(v);
            svg.line(ML, y, W - MR, y, GRID, 1.0, false);
            svg.text(ML - 8.0, y + 4.0, &self.label(v), 11.0, "end", MUTED, "normal");
            v += self.step;
        }
        svg.line(ML, MT, ML, H - MB, "#999", 1.0, false);
        svg.line(ML, H - MB, W - MR, H - MB, "#999", 1.0, false);
        let my = (MT + H - MB) / 2.0;
        svg.0 += &format!("<text x=\"20\" y=\"{my:.1}\" font-size=\"12\" text-anchor=\"middle\" fill=\"{MUTED}\" transform=\"rotate(-90 20 {my:.1})\">{}</text>\n", esc(ylabel));
    }
}

fn quantile(sorted: &[f64], q: f64) -> f64 {
    let k = (sorted.len() - 1) as f64 * q;
    let (f, c) = (k.floor() as usize, k.ceil() as usize);
    sorted[f] + (sorted[c] - sorted[f]) * (k - f as f64)
}

fn vfmt(x: f64) -> String { if x.abs() >= 100.0 { format!("{x:.0}") } else { format!("{x:.1}") } }

/// Ящики с усами (квартили, медиана, min…max, ◆ среднее) поверх всех наблюдений.
fn boxplot(title: &str, ylabel: &str, groups: &[(String, Vec<f64>)], source: &str) -> Option<String> {
    let all: Vec<f64> = groups.iter().flat_map(|g| g.1.iter().copied()).collect();
    if all.is_empty() { return None; }
    let axis = Axis::new(all.iter().copied().fold(f64::INFINITY, f64::min), all.iter().copied().fold(f64::NEG_INFINITY, f64::max));
    let mut svg = Svg::new(title, "ящик — квартили, черта — медиана (подписана), ◆ — среднее, усы — min…max, точки — наблюдения");
    axis.draw(&mut svg, ylabel);
    let slot = (W - ML - MR) / groups.len() as f64;
    let bw = (slot * 0.42).min(86.0);
    for (gi, (label, xs)) in groups.iter().enumerate() {
        let cx = ML + slot * (gi as f64 + 0.5);
        let color = COLORS[gi % COLORS.len()];
        svg.category(cx, label, Some(xs.len()));
        if xs.is_empty() {
            svg.text(cx, axis.y(axis.lo) - 10.0, "нет данных", 12.0, "middle", MUTED, "normal");
            continue;
        }
        let mut v = xs.clone();
        v.sort_by(|a, b| a.total_cmp(b));
        let (q1, med, q3) = (quantile(&v, 0.25), quantile(&v, 0.5), quantile(&v, 0.75));
        let st = stat(&v);
        svg.line(cx, axis.y(st.min), cx, axis.y(st.max), color, 1.4, false);
        for e in [st.min, st.max] { svg.line(cx - bw * 0.22, axis.y(e), cx + bw * 0.22, axis.y(e), color, 1.4, false); }
        svg.rect(cx - bw / 2.0, axis.y(q3), bw, axis.y(q1) - axis.y(q3), color, 0.18, color);
        for (i, x) in v.iter().enumerate() {
            let jitter = ((i as f64 * 0.618_034) % 1.0 - 0.5) * bw * 0.8;
            svg.circle(cx + jitter, axis.y(*x), 2.6, color, 0.55);
        }
        svg.line(cx - bw / 2.0, axis.y(med), cx + bw / 2.0, axis.y(med), INK, 2.4, false);
        svg.diamond(cx, axis.y(st.mean), 5.0);
        svg.text(cx + bw / 2.0 + 6.0, axis.y(med) + 4.0, &vfmt(med), 11.5, "start", INK, "bold");
    }
    Some(svg.end(source))
}

/// Столбцы: (подпись категории, значение, надпись над столбцом); опц. горизонтальная линия-ориентир.
fn bars(title: &str, subtitle: &str, ylabel: &str, items: &[(String, f64, String)], ymax: Option<f64>,
        hline: Option<(f64, String)>, source: &str) -> Option<String> {
    if items.is_empty() { return None; }
    let top = items.iter().map(|i| i.1).fold(hline.as_ref().map(|h| h.0).unwrap_or(0.0), f64::max);
    let axis = Axis::new(0.0, ymax.unwrap_or(top).max(top));
    let mut svg = Svg::new(title, subtitle);
    axis.draw(&mut svg, ylabel);
    let slot = (W - ML - MR) / items.len() as f64;
    let bw = (slot * 0.62).min(90.0);
    for (i, (label, v, text)) in items.iter().enumerate() {
        let cx = ML + slot * (i as f64 + 0.5);
        svg.rect(cx - bw / 2.0, axis.y(*v), bw, axis.y(0.0) - axis.y(*v), COLORS[0], 0.85, COLORS[0]);
        svg.text(cx, axis.y(*v) - 6.0, text, 11.5, "middle", INK, "bold");
        svg.category(cx, label, None);
    }
    if let Some((y, label)) = hline {
        // линия-ориентир; её подпись — в легенде над областью графика, чтобы не перекрывать столбцы
        svg.line(ML, axis.y(y), W - MR, axis.y(y), COLORS[3], 1.6, true);
        let tx = W - MR;
        svg.text(tx, 50.0, &label, 11.5, "end", COLORS[3], "bold");
        let lw = label.chars().count() as f64 * 6.8 + 8.0;
        svg.line(tx - lw - 26.0, 46.0, tx - lw, 46.0, COLORS[3], 1.6, true);
    }
    Some(svg.end(source))
}

/// Распределение дискретной величины (напр. число RPC): сколько наблюдений с каждым значением.
fn counts(title: &str, xlabel: &str, xs: &[f64], source: &str) -> Option<String> {
    let mut m: BTreeMap<i64, usize> = BTreeMap::new();
    for x in xs { *m.entry(x.round() as i64).or_default() += 1; }
    let items: Vec<(String, f64, String)> = m.iter().map(|(k, c)| (format!("{xlabel} {k}"), *c as f64, c.to_string())).collect();
    bars(title, &format!("распределение, n={}", xs.len()), "наблюдений", &items, None, None, source)
}

// ── графики по сценариям ─────────────────────────────────────────────────────

type Charts = Vec<(String, String, Option<String>)>;

fn g(label: &str, xs: Vec<f64>) -> (String, Vec<f64>) { (label.to_owned(), xs) }

fn cold(dir: &Path, topos: &[&str]) -> Result<Charts> {
    let rows = read_csv(&dir.join("results.csv"))?;
    let joins = read_csv(&dir.join("join.csv"))?;
    let conv: Vec<_> = topos.iter().map(|t| g(t, col(&rows, "converge_s", |r| eq(r, "topology", t) && yes(r, "converged")))).collect();
    let join: Vec<_> = topos.iter().map(|t| g(t, col(&joins, "join_ms", |r| eq(r, "topology", t)))).collect();
    Ok(vec![
        ("convergence.svg".into(), "Время операционной сходимости (§15.1), по запускам".into(),
         boxplot("Время операционной сходимости (§15.1)", "с от запуска первого процесса", &conv, "results.csv")),
        ("join_ms.svg".into(), "Bootstrap узла: PING bootstrap-узла + self-lookup".into(),
         boxplot("Bootstrap узла: PING bootstrap-узла + self-lookup", "мс (часы узла)", &join, "join.csv")),
    ])
}

fn s02(dir: &Path) -> Result<Charts> {
    let rows = read_csv(&dir.join("results.csv"))?;
    let found = |r: &Row| yes(r, "found");
    Ok(vec![
        ("duration_ms.svg".into(), "Длительность lookup, цель вне таблицы инициатора".into(),
         boxplot("FIND_NODE: цель отсутствует в таблице инициатора", "мс", &[g("lookup", col(&rows, "duration_ms", found))], "results.csv")),
        ("rpc_count.svg".into(), "Число RPC на lookup".into(),
         counts("Число FIND_NODE RPC на один lookup", "RPC", &col(&rows, "rpc_count", found), "results.csv")),
        ("iterations.svg".into(), "Число итераций lookup".into(),
         counts("Число итераций на один lookup", "итераций", &col(&rows, "iterations", found), "results.csv")),
    ])
}

fn s03(dir: &Path) -> Result<Charts> {
    let rows = read_csv(&dir.join("results.csv"))?;
    let groups = [
        g("STORE\n(lookup + 3 реплики)", col(&rows, "store_ms", |r| yes(r, "store_ok"))),
        g("FIND_VALUE\nчитатель не хранитель", col(&rows, "find_ms", |r| yes(r, "found_ok") && !yes(r, "reader_is_holder"))),
        g("FIND_VALUE\nчитатель-хранитель", col(&rows, "find_ms", |r| yes(r, "found_ok") && yes(r, "reader_is_holder"))),
    ];
    Ok(vec![("latency.svg".into(), "STORE и FIND_VALUE, R=3".into(),
             boxplot("STORE / FIND_VALUE, R=3 (время запроса к API узла)", "мс", &groups, "results.csv"))])
}

fn s04(dir: &Path) -> Result<Charts> {
    let rows = read_csv(&dir.join("results.csv"))?;
    let mut items = vec![];
    for k in ["1", "2", "3"] {
        let sub: Vec<&Row> = rows.iter().filter(|r| eq(r, "kill_planned", k)).collect();
        if sub.is_empty() { continue; }
        let ok = sub.iter().filter(|r| yes(r, "found_ok")).count();
        items.push((format!("отключено\nхранителей: {k}"), 100.0 * ok as f64 / sub.len() as f64, format!("{ok}/{}", sub.len())));
    }
    let times = vec![g("FIND_VALUE после отказа\n1 хранителя", col(&rows, "find_ms", |r| eq(r, "kill_planned", "1") && yes(r, "found_ok")))];
    Ok(vec![
        ("found_rate.svg".into(), "Доля найденных записей после отключения хранителей".into(),
         bars("FIND_VALUE после отключения хранителей (R=3)", "автор записи не отключается; читает узел, не хранящий запись",
              "% найдено", &items, Some(100.0), None, "results.csv")),
        ("find_ms.svg".into(), "Время FIND_VALUE после отказа одного хранителя".into(),
         boxplot("FIND_VALUE после отказа одного хранителя", "мс", &times, "results.csv")),
    ])
}

fn s05(dir: &Path) -> Result<Charts> {
    let rows = read_csv(&dir.join("results.csv"))?;
    let mut fr: Vec<String> = rows.iter().filter_map(|r| r.get("fail_fraction").cloned()).collect();
    fr.dedup();
    let label = |f: &str| {
        let r = rows.iter().find(|r| eq(r, "fail_fraction", f));
        let pct = f.parse::<f64>().map(|x| (x * 100.0).round()).unwrap_or(0.0);
        format!("отключено {pct:.0}%\n(живых {})", r.and_then(|r| r.get("alive")).map(String::as_str).unwrap_or("?"))
    };
    let groups: Vec<_> = fr.iter().map(|f| g(&label(f), col(&rows, "duration_ms", |r| eq(r, "fail_fraction", f) && yes(r, "found")))).collect();
    let items: Vec<_> = fr.iter().map(|f| {
        let sub: Vec<&Row> = rows.iter().filter(|r| eq(r, "fail_fraction", f)).collect();
        let ok = sub.iter().filter(|r| yes(r, "found")).count();
        (label(f), 100.0 * ok as f64 / sub.len().max(1) as f64, format!("{ok}/{}", sub.len()))
    }).collect();
    Ok(vec![
        ("success.svg".into(), "Доля успешных lookup при отключении узлов".into(),
         bars("Успешные lookup при отключении части узлов", "отключённые узлы ещё числятся в таблицах маршрутизации",
              "% найдено", &items, Some(100.0), Some((95.0, "95% (критерий §15.1)".into())), "results.csv")),
        ("duration_ms.svg".into(), "Длительность lookup при отключении узлов".into(),
         boxplot("Длительность успешного lookup при отключении узлов", "мс", &groups, "results.csv")),
    ])
}

fn s06(dir: &Path) -> Result<Charts> {
    let rows = read_csv(&dir.join("results.csv"))?;
    let c = |k: &str, kind: &str| col(&rows, k, |r| yes(r, "delivered") && eq(r, "kind", kind));
    Ok(vec![
        ("rtt.svg".into(), "RTT до APP_ACK: первое сообщение и установленная сессия".into(),
         boxplot("Задержка сообщения до подтверждения получателем (APP_ACK)", "мс",
                 &[g("первое сообщение\n(lookup + туннели)", c("rtt_ms", "first")), g("установленная\nсессия", c("rtt_ms", "session"))], "results.csv")),
        ("components.svg".into(), "Составляющие задержки".into(),
         boxplot("Составляющие задержки", "мс",
                 &[g("lookup получателя\n(первое)", c("lookup_ms", "first")), g("построение\nтуннеля", c("tunnel_build_ms", "first")),
                   g("проход по цепочке\nrelay (сессия)", c("tunnel_send_ms", "session"))], "results.csv")),
    ])
}

fn s07(dir: &Path) -> Result<Charts> {
    let rows = read_csv(&dir.join("results.csv"))?;
    let rec = |r: &Row| eq(r, "outcome", "recovered");
    Ok(vec![("recovery.svg".into(), "Восстановление доставки после отказа relay".into(),
             boxplot("Отказ активного ретранслятора: восстановление доставки", "мс от отключения relay",
                     &[g("до APP_ACK\n(обнаружение + новый туннель)", col(&rows, "recovery_ms", rec)),
                       g("из них построение\nнового туннеля", col(&rows, "rebuild_ms", rec))], "results.csv"))])
}

fn channel(dir: &Path) -> Result<Charts> {
    let rows = read_csv(&dir.join("results.csv"))?;
    let c = |ch: &str, m: &str| col(&rows, "value_ms", |r| eq(r, "channel", ch) && eq(r, "metric", m));
    let pair = |m: &str| vec![g("TLS 1.3", c("tls", m)), g("AKE (§11.3)", c("ake", m))];
    Ok(vec![
        ("ping.svg".into(), "Новое соединение + 1 RPC: TLS и AKE".into(),
         boxplot("PING по новому соединению: рукопожатие + 1 RPC", "мс", &pair("ping"), "results.csv")),
        ("lookup.svg".into(), "Lookup: TLS и AKE".into(), boxplot("Длительность lookup", "мс", &pair("lookup"), "results.csv")),
        ("session.svg".into(), "RTT сессии: TLS и AKE".into(),
         boxplot("Установленная сессия: RTT до APP_ACK", "мс", &pair("session"), "results.csv")),
        ("first.svg".into(), "Первое сообщение: TLS и AKE".into(),
         boxplot("Первое сообщение: RTT до APP_ACK", "мс", &pair("first"), "results.csv")),
    ])
}

fn files(dir: &Path) -> Result<Charts> {
    let rows = read_csv(&dir.join("results.csv"))?;
    let mut labels: Vec<String> = rows.iter().filter_map(|r| r.get("size_label").cloned()).collect();
    labels.dedup();
    let ok = |r: &Row| yes(r, "ok") && yes(r, "sha256_verified");
    let gp: Vec<_> = labels.iter().map(|l| g(l, col(&rows, "goodput_mib_s", |r| eq(r, "size_label", l) && ok(r)))).collect();
    let oh: Vec<_> = labels.iter().map(|l| {
        let v = col(&rows, "overhead_share", |r| eq(r, "size_label", l) && ok(r));
        let m = stat(&v).median * 100.0;
        (l.clone(), if m.is_finite() { m } else { 0.0 }, if m.is_finite() { format!("{m:.2}%") } else { "—".into() })
    }).collect();
    Ok(vec![
        ("goodput.svg".into(), "Goodput передачи файла по размерам".into(),
         boxplot("Передача файла: goodput", "МиБ/с", &gp, "results.csv")),
        ("overhead.svg".into(), "Доля служебного трафика прикладного протокола".into(),
         bars("Служебная доля трафика (медиана)", "подпись, конверт, метаданные и подтверждения; без TLS/TCP",
              "% от переданных байт", &oh, None, None, "results.csv")),
    ])
}

fn failover(dir: &Path) -> Result<Charts> {
    let rows = read_csv(&dir.join("results.csv"))?;
    let groups = vec![
        g("обнаружение отказа\n(переключение)", col(&rows, "detect_ms", |r| eq(r, "outcome", "failover"))),
        g("переключение\nна резервный", col(&rows, "switch_ms", |r| eq(r, "outcome", "failover"))),
        g("обнаружение отказа\n(перестроение)", col(&rows, "detect_ms", |r| eq(r, "outcome", "rebuild"))),
        g("восстановление\nдо APP_ACK", col(&rows, "recovery_ms", |r| yes(r, "delivered"))),
    ];
    let groups: Vec<_> = groups.into_iter().filter(|g| !g.1.is_empty()).collect();
    Ok(vec![("failover.svg".into(), "Отказ relay основного туннеля: обнаружение и переключение".into(),
             boxplot("Отказ ретранслятора основного туннеля", "мс", &groups, "results.csv"))])
}

fn strategy(dir: &Path) -> Result<Charts> {
    let rows = read_csv(&dir.join("results.csv"))?;
    let sts = ["closest", "random", "rating"];
    let items: Vec<_> = sts.iter().map(|st| {
        let m: Vec<&Row> = rows.iter().filter(|r| eq(r, "strategy", st) && eq(r, "phase", "measure")).collect();
        let clean = m.iter().filter(|r| yes(r, "clean")).count();
        (st.to_string(), 100.0 * clean as f64 / m.len().max(1) as f64, format!("{clean}/{}", m.len()))
    }).collect();
    let rtt: Vec<_> = sts.iter().map(|st| g(st, col(&rows, "rtt_ms", |r| eq(r, "strategy", st) && eq(r, "phase", "measure") && yes(r, "delivered")))).collect();
    Ok(vec![
        ("clean.svg".into(), "Доставки без отказа на пути по стратегиям".into(),
         bars("Доставлено первым же туннелем, без отказа на пути", "четверть ретрансляторов неисправна (отказ с вероятностью 0,5)",
              "% сообщений", &items, Some(100.0), None, "results.csv")),
        ("rtt.svg".into(), "RTT до APP_ACK по стратегиям".into(), boxplot("RTT до APP_ACK по стратегиям выбора", "мс", &rtt, "results.csv")),
    ])
}

fn netem(dir: &Path) -> Result<Charts> {
    let rows = read_csv(&dir.join("results.csv"))?;
    let mut prof: Vec<String> = rows.iter().filter_map(|r| r.get("profile").cloned()).collect();
    prof.dedup();
    let by = |m: &str| -> Vec<(String, Vec<f64>)> { prof.iter().map(|p| g(p, col(&rows, "value_ms", |r| eq(r, "profile", p) && eq(r, "metric", m) && yes(r, "ok")))).collect() };
    Ok(vec![
        ("lookup.svg".into(), "Lookup при деградации underlay".into(), boxplot("Lookup под tc/netem", "мс", &by("lookup"), "results.csv")),
        ("session.svg".into(), "RTT сессии при деградации underlay".into(), boxplot("RTT сессии под tc/netem", "мс", &by("message_session"), "results.csv")),
    ])
}

/// Приёмка этапов 1–2: графики + stats.md (замена scripts/python/analyze.py).
fn accept(dir: &Path) -> Result<Charts> {
    let before = read_csv(&dir.join("lookups_before.csv"))?;
    let after = read_csv(&dir.join("lookups_after_bootstrap_loss.csv"))?;
    let routing = read_csv(&dir.join("routing_summary.csv"))?;
    let name = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let mut md = format!("# Статистика прогона `{name}`\n\np95 — только при n ≥ 20 (ТЗ §15.3).\n\n| Метрика | n | min | среднее | медиана | p95 | max |\n|---|---|---|---|---|---|---|\n");
    let row = |title: &str, xs: &[f64]| {
        let s = stat(xs);
        if s.n == 0 { return format!("| {title} | 0 | — | — | — | — | — |\n"); }
        format!("| {title} | {} | {:.1} | {:.2} | {:.1} | {} | {:.1} |\n", s.n, s.min, s.mean, s.median,
                s.p95.map(|p| format!("{p:.1}")).unwrap_or("—".into()), s.max)
    };
    let mut charts = vec![];
    for (metric, title, unit) in [("duration_ms", "Длительность lookup", "мс"), ("rpc_count", "Число RPC на lookup", "RPC"), ("iterations", "Итерации lookup", "итераций")] {
        let (b, a) = (col(&before, metric, |_| true), col(&after, metric, |_| true));
        md += &row(&format!("{title}, до отказа bootstrap"), &b);
        md += &row(&format!("{title}, после отказа bootstrap"), &a);
        charts.push((format!("{metric}.svg"), title.to_owned(),
                     boxplot(title, unit, &[g("до отказа\nbootstrap", b), g("после отказа\nbootstrap", a)], "lookups_*.csv")));
    }
    if !routing.is_empty() {
        let sizes: Vec<f64> = col(&routing, "total_contacts", |_| true);
        let items: Vec<_> = routing.iter().map(|r| {
            let v = num(r, "total_contacts").unwrap_or(0.0);
            (r.get("port").cloned().unwrap_or_default(), v, format!("{v:.0}"))
        }).collect();
        let n1 = routing.len() as f64 - 1.0;
        charts.push(("routing_table_sizes.svg".into(), "Размер таблиц маршрутизации".into(),
                     bars("Размер таблицы маршрутизации после сходимости", "у каждого узла меньше N−1 контактов — сеть не вырождена",
                          "контактов", &items, None, Some((n1, format!("N−1 = {n1:.0}"))), "routing_summary.csv")));
        let s = stat(&sizes);
        md += &format!("\nТаблицы маршрутизации: узлов {}, контактов min/среднее/max = {:.0}/{:.1}/{:.0} (N−1 = {n1:.0}).\n", s.n, s.min, s.mean, s.max);
    }
    std::fs::write(dir.join("stats.md"), md)?;
    Ok(charts)
}

/// Построить графики каталога результатов в `<каталог>/plots/`. Возвращает (файл, подпись).
pub fn plot_dir(dir: &Path) -> Result<Vec<(String, String)>> {
    let name = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let scen = dir.parent().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let charts = if name.starts_with("accept") { accept(dir)? } else {
        match scen.get(..2).unwrap_or("") {
            "01" => cold(dir, &["star"])?,
            "09" => {
                let rows = read_csv(&dir.join("results.csv"))?;
                let mut t: Vec<String> = rows.iter().filter_map(|r| r.get("topology").cloned()).collect();
                t.dedup();
                cold(dir, &t.iter().map(String::as_str).collect::<Vec<_>>())?
            }
            "02" => s02(dir)?,
            "03" => s03(dir)?,
            "04" => s04(dir)?,
            "05" => s05(dir)?,
            "06" => s06(dir)?,
            "07" => s07(dir)?,
            _ if scen == "channel_compare" => channel(dir)?,
            _ if scen == "files_transfer" => files(dir)?,
            _ if scen == "failover" => failover(dir)?,
            _ if scen == "relay_strategy" => strategy(dir)?,
            "10" => netem(dir)?,
            _ => vec![],
        }
    };
    let mut out = vec![];
    for (file, caption, svg) in charts {
        let Some(svg) = svg else { continue };
        std::fs::create_dir_all(dir.join("plots"))?;
        std::fs::write(dir.join("plots").join(&file), svg)?;
        out.push((file, caption));
    }
    Ok(out)
}
