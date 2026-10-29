#!/usr/bin/env python3
"""scripts/scenarios.py — реальные измерения сценариев 01–09 (ТЗ §15.1, §22: без синтетики).

Запускает настоящие процессы `p2p-node`, управляет ими через HTTP API панели узла
и пишет в experiments/results/<сценарий>/<timestamp>/:
  results.csv  — сырые наблюдения (строка = наблюдение);
  summary.md   — n / min / max / медиана / среднее (p95 только при n ≥ 20, ТЗ §15.3);
  meta.json    — версия кода, конфигурация, seed, время старта, временная база,
                 N, k, α, R, тайм-ауты, граф bootstrap (ТЗ §15.3).
Использование:
  python3 scripts/scenarios.py <01..09|pcap|all> [--nodes N] [--n OBS] [--runs R] [--inj I] [--seed S]
Длительности меряются оркестратором по time.monotonic(); поля *_ms из API — часами узла.
"""
import argparse, csv, json, math, os, random, re, signal, statistics, subprocess, sys, time
import urllib.request, urllib.parse
import hashlib, shutil, uuid

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
BIN = os.path.join(ROOT, "target/debug/p2p-node")
CFG = os.path.join(ROOT, "config/node_config.yaml")
_port = [7400]


def http(gui, method, path, timeout=40, **q):
    url = f"http://127.0.0.1:{gui}{path}"
    if q:
        url += "?" + urllib.parse.urlencode(q)
    req = urllib.request.Request(url, method=method, data=b"" if method == "POST" else None)
    with urllib.request.urlopen(req, timeout=timeout) as r:
        return json.loads(r.read())


def node_config():
    """Плоский разбор config/node_config.yaml (секция → ключ: значение), без PyYAML."""
    cfg, sec = {}, None
    for raw in open(CFG):
        line = raw.split("#", 1)[0].rstrip()
        if not line.strip():
            continue
        k, _, v = line.strip().partition(":")
        v = v.strip()
        if not line.startswith(" "):
            sec = k
            cfg[k] = {} if not v else v
        elif isinstance(cfg.get(sec), dict):
            cfg[sec][k] = int(v) if v.isdigit() else v
    return cfg


class Net:
    """Набор живых узлов. topology: star (все к seed) | ring (i → i-1)."""

    def __init__(self, tag, n, topology="star"):
        self.dir = os.path.join(ROOT, "logs", "scenarios", f"{tag}_{int(time.time())}")
        os.makedirs(self.dir, exist_ok=True)
        self.n, self.topology = n, topology
        self.procs, self.ports, self.args = [], [], []
        base = _port[0]; _port[0] += 2 * n + 4
        self.t_spawn = time.monotonic()
        for i in range(n):
            p = base + i
            args = [BIN, "-p", str(p), "--gui-port", str(p + n + 2),
                    "--state-dir", f"{self.dir}/state{i}", "--metrics-dir", f"{self.dir}/metrics{i}"]
            if i > 0:
                b = base if topology == "star" else base + i - 1
                args += ["-b", f"127.0.0.1:{b}"]
            self.args.append(args)
            self.ports.append(p)
            self._spawn(i)
            time.sleep(0.5 if i == 0 else 0.15)
        self.launch_span_s = time.monotonic() - self.t_spawn
        self.gui = [p + n + 2 for p in self.ports]
        self.ids = [self._wait_id(g) for g in self.gui]

    def _spawn(self, i):
        log = open(f"{self.dir}/n{i}.log", "a")
        p = subprocess.Popen(self.args[i], stdout=log, stderr=subprocess.STDOUT, cwd=ROOT)
        if i < len(self.procs):
            self.procs[i] = p
        else:
            self.procs.append(p)

    def _wait_id(self, g):
        for _ in range(100):
            try:
                return http(g, "GET", "/api/status", timeout=5)["node_id"]
            except Exception:
                time.sleep(0.2)
        raise RuntimeError(f"node gui {g} не поднялся")

    def status(self, i):
        return http(self.gui[i], "GET", "/api/status", timeout=10)

    def contacts(self, i):
        try:
            return self.status(i)["routing"]["total_contacts"]
        except Exception:
            return -1

    def contact_ids(self, i):
        rt = self.status(i)["routing"]
        return {c["node_id"] for b in rt["buckets"] for c in b["contacts"]}

    def converge(self, timeout=60, stable=3):
        """Подготовка (не метрика): ждём, пока сумма контактов не меняется `stable` опросов подряд."""
        last, same, t0 = None, 0, time.monotonic()
        while time.monotonic() - t0 < timeout:
            s = sum(max(self.contacts(i), 0) for i in self.alive())
            same = same + 1 if s == last else 0
            last = s
            if same >= stable:
                return time.monotonic() - t0
            time.sleep(1)
        return float("nan")

    def alive(self):
        return [i for i, p in enumerate(self.procs) if p.poll() is None]

    def kill(self, i):
        self.procs[i].kill(); self.procs[i].wait()

    def restart(self, i):
        """Перезапуск с той же identity и портом; bootstrap — любой живой узел."""
        if self.procs[i].poll() is None:
            self.kill(i)
        peer = next(j for j in self.alive() if j != i)
        args = [x for x in self.args[i]]
        if "-b" in args:
            k = args.index("-b"); del args[k:k + 2]
        self.args[i] = args + ["-b", f"127.0.0.1:{self.ports[peer]}"]
        self._spawn(i)
        self._wait_id(self.gui[i])

    def stop(self):
        for p in self.procs:
            if p.poll() is None:
                p.send_signal(signal.SIGTERM)
        for p in self.procs:
            try: p.wait(5)
            except Exception: p.kill()


def r1(x):
    return round(float(x), 1)


def stat(xs):
    """n / min / max / медиана / среднее; p95 только при n ≥ 20 (ТЗ §15.3)."""
    xs = sorted(float(x) for x in xs if x is not None and x != "" and x == x)
    if not xs:
        return {"n": 0}
    n = len(xs)
    d = {"n": n, "min": r1(xs[0]), "max": r1(xs[-1]),
         "median": r1(statistics.median(xs)), "mean": r1(statistics.mean(xs))}
    if n >= 20:
        d["p95"] = r1(xs[math.ceil(0.95 * n) - 1])
    return d


def fmt(d):
    if not d.get("n"):
        return "n=0"
    s = f"n={d['n']}, min {d['min']}, медиана {d['median']}, среднее {d['mean']}, max {d['max']}"
    return s + (f", p95 {d['p95']}" if "p95" in d else "")


def git(*args):
    return subprocess.run(["git", *args], cwd=ROOT, capture_output=True, text=True).stdout.strip()


def worktree_hash():
    if not git("status", "--porcelain"):
        return None
    h = hashlib.sha256(subprocess.run(["git", "diff", "HEAD", "--binary"], cwd=ROOT, capture_output=True).stdout)
    for f in sorted(git("ls-files", "--others", "--exclude-standard").splitlines()):
        if f.startswith("experiments/results/"):
            continue
        h.update(f.encode()); h.update(open(os.path.join(ROOT, f), "rb").read())
    return h.hexdigest()


class Run:
    """Каталог результатов одного сценария + meta.json (ТЗ §15.3)."""

    def __init__(self, name, a, topology="star", **params):
        self.name = name
        self.dir = os.path.join(ROOT, "experiments/results", name, time.strftime("%Y%m%d_%H%M%S"))
        os.makedirs(self.dir, exist_ok=True)
        random.seed(a.seed)
        cfg = node_config()
        self.meta = {
            "scenario": name,
            "commit": git("rev-parse", "HEAD"),
            "worktree_dirty": bool(git("status", "--porcelain")),
            # точная версия кода при незакоммиченных изменениях: хэш diff (с новыми файлами)
            "worktree_diff_sha256": worktree_hash(),
            "started_at": time.strftime("%Y-%m-%dT%H:%M:%S%z"),
            "timebase": "длительности: time.monotonic() оркестратора; *_ms из API — часы узла (unix ms)",
            "seed": a.seed,
            "N": a.nodes,
            "k": cfg.get("dht", {}).get("k_bucket_size"),
            "alpha": cfg.get("dht", {}).get("alpha"),
            "R": cfg.get("dht", {}).get("replication_factor"),
            "timeouts_ms": {k: v for sec in ("dht", "transport", "tunnel") for k, v in cfg.get(sec, {}).items()
                            if k.endswith("_ms")},
            "tunnel": cfg.get("tunnel"),
            "bootstrap_graph": topology,
            "config_file": os.path.relpath(CFG, ROOT),
            "config": cfg,
            "params": params,
            "netem": None,
        }

    def write_csv(self, fname, header, rows):
        with open(f"{self.dir}/{fname}", "w", newline="") as f:
            w = csv.writer(f); w.writerow(header); w.writerows(rows)

    def finish(self, header, rows, summary):
        self.write_csv("results.csv", header, rows)
        self.meta["finished_at"] = time.strftime("%Y-%m-%dT%H:%M:%S%z")
        with open(f"{self.dir}/meta.json", "w") as f:
            json.dump(self.meta, f, ensure_ascii=False, indent=1)
        m = self.meta
        head = (f"коммит `{m['commit']}`{' + незакоммиченные изменения (diff sha256 `' + m['worktree_diff_sha256'][:12] + '`)' if m['worktree_dirty'] else ''}, "
                f"N={m['N']}, k={m['k']}, α={m['alpha']}, R={m['R']}, bootstrap={m['bootstrap_graph']}, seed={m['seed']}")
        with open(f"{self.dir}/summary.md", "w") as f:
            f.write(f"# {self.name}\n\n{head}\n\n{summary}\n")
        print(f"== {self.name}: {self.dir}\n{summary}")


def lookup(net, i, target):
    return http(net.gui[i], "POST", "/api/lookup", target=target)


def send(net, s, r, text):
    try:
        return http(net.gui[s], "POST", "/api/send", target=net.ids[r], text=text)
    except Exception as e:
        return {"ok": False, "delivered": False, "error": f"http: {e}"}


# ── 01 / 09: операционная сходимость (ТЗ §15.1) ──────────────────────────────

ROUND_PAUSE_S = 1.0
CONVERGE_TIMEOUT_S = 120


def op_converge(net, plan):
    """Контрольные раунды по заранее сформированному набору (initiator, target).
    Сходимость — когда два раунда подряд дают ≥95% успешных lookup, а суммарное число
    контактов между ними меняется ≤10%. Время — от запуска первого процесса до конца
    второго подтверждающего раунда. Узлам набор не передаётся."""
    rounds, prev = [], None
    for rnd, pairs in enumerate(plan):
        ok = 0
        for i, t in pairs:
            try:
                r = lookup(net, i, net.ids[t])
                ok += bool(r.get("ok") and r.get("found_target"))
            except Exception:
                pass
        cs = [net.contacts(i) for i in range(net.n)]
        cur = (time.monotonic() - net.t_spawn, ok / len(pairs), sum(max(c, 0) for c in cs), cs)
        rounds.append(cur)
        if prev and prev[1] >= 0.95 and cur[1] >= 0.95 and abs(cur[2] - prev[2]) <= 0.10 * max(prev[2], 1):
            return True, cur[0], rounds
        if cur[0] > CONVERGE_TIMEOUT_S:
            break
        prev = cur
        time.sleep(ROUND_PAUSE_S)
    return False, float("nan"), rounds


def cold_runs(a, topo, runs):
    rows, rrows, jrows = [], [], []
    for k in range(1, runs + 1):
        net = Net(f"cold_{topo}", a.nodes, topo)
        try:
            n = net.n
            plan = [[(i, random.choice([j for j in range(n) if j != i])) for i in range(n)] for _ in range(200)]
            ok, t, rounds = op_converge(net, plan)
            for j, (ts, succ, csum, _) in enumerate(rounds):
                rrows.append([topo, k, j, r1(ts), round(succ, 3), csum])
            cs = rounds[-1][3]
            joins = []
            for i in range(1, n):  # seed (узел 0) bootstrap не выполняет
                b = net.status(i).get("bootstrap") or {}
                if b.get("join_ms") is not None:
                    joins.append(b["join_ms"]); jrows.append([topo, k, i, b["join_ms"]])
            rows.append([topo, k, ok, r1(t) if ok else "", len(rounds), r1(net.launch_span_s),
                         round(rounds[-1][1], 3), sum(cs), min(cs), max(cs),
                         r1(statistics.median(joins)) if joins else "", max(joins) if joins else ""])
        finally:
            net.stop()
    return rows, rrows, jrows


COLD_HDR = ["topology", "run", "converged", "converge_s", "rounds", "launch_span_s",
            "last_round_success", "contacts_sum", "contacts_min", "contacts_max", "join_ms_median", "join_ms_max"]


def cold_line(rows, jrows, topo):
    sub = [r for r in rows if r[0] == topo]
    conv = [r[3] for r in sub if r[2]]
    joins = [j[3] for j in jrows if j[0] == topo]
    return (f"- **{topo}**: сошлось {len(conv)}/{len(sub)}; время операционной сходимости, с: {fmt(stat(conv))}; "
            f"контактов на узел (min…max по прогонам): {min(r[8] for r in sub)}…{max(r[9] for r in sub)}, "
            f"среднее суммарное {r1(statistics.mean(r[7] for r in sub))}\n"
            f"  - bootstrap узла (по часам узла: от старта процесса до конца PING bootstrap-узла + self-lookup), мс: {fmt(stat(joins))}")


def s01(a):
    run = Run("01_cold_start", a, "star", runs=a.runs, round_pause_s=ROUND_PAUSE_S)
    rows, rrows, jrows = cold_runs(a, "star", a.runs)
    run.write_csv("rounds.csv", ["topology", "run", "round", "t_s", "success", "contacts_sum"], rrows)
    run.write_csv("join.csv", ["topology", "run", "node", "join_ms"], jrows)
    run.finish(COLD_HDR, rows,
               f"Холодный запуск, {a.runs} независимых запусков. Критерий §15.1: два контрольных раунда подряд "
               f"(по {a.nodes} lookup из заранее сформированного набора) с успехом ≥95% и изменением суммы "
               f"контактов ≤10% (окно наблюдения — пауза {ROUND_PAUSE_S} с между раундами); отсчёт — от запуска "
               f"первого процесса. Процессы запускаются с шагом 0.15 с (launch_span_s), поэтому нижняя граница "
               f"метода ≈ launch_span_s + 2 раунда + окно.\n\n"
               + cold_line(rows, jrows, "star") + "\n\nПо раундам — rounds.csv, bootstrap каждого узла — join.csv.")


def s09(a):
    run = Run("09_bootstrap_compare", a, "star+ring", runs=a.runs, round_pause_s=ROUND_PAUSE_S)
    rows, rrows, jrows = [], [], []
    for topo in ("star", "ring"):
        r, rr, jr = cold_runs(a, topo, a.runs)
        rows += r; rrows += rr; jrows += jr
    run.write_csv("rounds.csv", ["topology", "run", "round", "t_s", "success", "contacts_sum"], rrows)
    run.write_csv("join.csv", ["topology", "run", "node", "join_ms"], jrows)
    run.finish(COLD_HDR, rows,
               f"Сравнение bootstrap-схем при прочих равных (N, k, α, R, тайм-ауты, seed): star — все узлы "
               f"к seed, ring — узел i к узлу i−1. {a.runs} запусков на схему, критерий сходимости как в 01.\n\n"
               + cold_line(rows, jrows, "star") + "\n" + cold_line(rows, jrows, "ring"))


# ── 02: FIND_NODE для цели, отсутствующей у инициатора ───────────────────────

def s02(a):
    run = Run("02_find_node", a, lookups=a.n)
    net = Net("s02", a.nodes); rows = []
    try:
        net.converge()
        durs, rpcs, its = [], [], []
        for k in range(a.n):
            for _ in range(50):
                i = random.randrange(net.n)
                known = net.contact_ids(i)
                cands = [j for j in range(net.n) if j != i and net.ids[j] not in known]
                if cands:
                    break
            t = random.choice(cands)
            r = lookup(net, i, net.ids[t])
            ok = bool(r.get("ok") and r.get("found_target"))
            rows.append([k, i, t, net.ids[t][:8], r.get("target_known_before"), ok,
                         r.get("rpc_count"), r.get("iterations"), r.get("duration_ms")])
            if ok:
                durs.append(r["duration_ms"]); rpcs.append(r["rpc_count"]); its.append(r["iterations"])
        found = sum(1 for r in rows if r[5])
        absent = sum(1 for r in rows if r[4] is False)
        run.finish(["n", "initiator", "target", "target_id", "known_before", "found", "rpc_count", "iterations", "duration_ms"], rows,
                   f"Lookup {len(rows)}; цель отсутствовала в таблице инициатора (по данным узла): {absent}/{len(rows)}; "
                   f"найдено {found}/{len(rows)} ({100 * found // max(len(rows), 1)}%).\n\n"
                   f"- duration_ms: {fmt(stat(durs))}\n- RPC на lookup: {fmt(stat(rpcs))}\n- итераций: {fmt(stat(its))}")
    finally:
        net.stop()


# ── 03: STORE / FIND_VALUE, R=3 ──────────────────────────────────────────────

def key_hex(key):
    return hashlib.sha256(key.encode()).hexdigest()


def holders(net, key):
    """Узлы, у которых в хранилище есть запись (ключ DHT = SHA-256(строки))."""
    kh, out = key_hex(key), []
    for i in net.alive():
        try:
            if any(r["key"] == kh for r in net.status(i)["store"]["records"]):
                out.append(i)
        except Exception:
            pass
    return out


def s03(a):
    run = Run("03_store_find_value", a, operations=a.n)
    net = Net("s03", a.nodes); rows = []
    try:
        net.converge(); st, fd, fd_remote = [], [], []
        for k in range(a.n):
            w, r = random.sample(range(net.n), 2); key, val = f"s03-{k}-{uuid.uuid4().hex[:8]}", f"v{k}"
            t0 = time.monotonic(); s = http(net.gui[w], "POST", "/api/store", key=key, value=val)
            tw = (time.monotonic() - t0) * 1000
            hs = holders(net, key)
            local = r in hs
            t0 = time.monotonic(); f = http(net.gui[r], "GET", "/api/find", key=key)
            tr = (time.monotonic() - t0) * 1000
            ok = bool(f.get("found")) and f.get("value") == val
            rows.append([k, w, r, s.get("ok"), s.get("stored_on"), len(hs), local, r1(tw), ok, r1(tr)])
            if s.get("ok"): st.append(tw)
            if ok:
                fd.append(tr)
                if not local: fd_remote.append(tr)
        okc = sum(1 for x in rows if x[8])
        reps = [x[5] for x in rows]
        run.finish(["n", "writer", "reader", "store_ok", "stored_on", "holders", "reader_is_holder", "store_ms", "found_ok", "find_ms"], rows,
                   f"Операций {len(rows)}; STORE успешен {sum(1 for x in rows if x[3])}; значение прочитано верно {okc}/{len(rows)}.\n"
                   f"stored_on — число подтвердивших узлов включая автора (R=3 реплики + локальная копия); holders — фактически "
                   f"найдено в хранилищах по /api/status: {fmt(stat(reps))}.\n\n"
                   f"- STORE, мс (lookup ключа + 3×STORE): {fmt(stat(st))}\n- FIND_VALUE, мс (все): {fmt(stat(fd))}\n"
                   f"- FIND_VALUE, мс (читатель не хранитель): {fmt(stat(fd_remote))}")
    finally:
        net.stop()


# ── 04: отказ хранителя ──────────────────────────────────────────────────────

def s04(a):
    plan = [1] * a.inj + [2] * 3 + [3] * 3
    run = Run("04_keeper_failure", a, injections=plan)
    net = Net("s04", a.nodes); rows = []
    try:
        net.converge(); writer = 0
        for k, kill_n in enumerate(plan):
            key, val = f"s04-{k}-{uuid.uuid4().hex[:8]}", f"keep{k}"
            s = http(net.gui[writer], "POST", "/api/store", key=key, value=val)
            hs = [h for h in holders(net, key) if h != writer]
            victims = random.sample(hs, min(kill_n, len(hs)))
            for v in victims: net.kill(v)
            time.sleep(1)
            readers = [i for i in net.alive() if i != writer and i not in hs]
            r = random.choice(readers)
            t0 = time.monotonic(); f = http(net.gui[r], "GET", "/api/find", key=key); ms = (time.monotonic() - t0) * 1000
            ok = bool(f.get("found")) and f.get("value") == val
            rows.append([k, kill_n, s.get("stored_on"), len(hs), len(victims), len(hs) - len(victims), r, ok, r1(ms)])
            for v in victims: net.restart(v)
            net.converge(timeout=20, stable=2)
        lines = []
        for kn in (1, 2, 3):
            sub = [x for x in rows if x[1] == kn]
            if sub:
                lines.append(f"- отключено хранителей {kn}: найдено {sum(1 for x in sub if x[7])}/{len(sub)}; "
                             f"find_ms: {fmt(stat([x[8] for x in sub if x[7]]))}")
        run.finish(["n", "kill_planned", "stored_on", "holders_excl_writer", "killed", "holders_left", "reader", "found_ok", "find_ms"], rows,
                   "Запись публикует узел 0; отключаются (SIGKILL) хранители-реплики (автор не трогается), "
                   "читает живой узел, не хранящий запись. После каждой инъекции отключённые узлы перезапускаются "
                   "(та же identity) — условия следующей инъекции одинаковы. Основной сценарий §15.1 п.4 — "
                   "отключение одного хранителя; 2 и 3 — граница отказоустойчивости при R=3.\n\n" + "\n".join(lines))
    finally:
        net.stop()


# ── 05: отказ промежуточных кандидатов lookup ────────────────────────────────

def s05(a):
    fracs = (0, 0.2, 0.4)
    run = Run("05_lookup_failure", a, fail_fractions=fracs, lookups_per_level=a.n, wait_after_kill_s=2)
    net = Net("s05", a.nodes); rows = []
    try:
        net.converge()
        order = list(range(1, net.n)); random.shuffle(order)
        for frac in fracs:
            killed = order[:int(frac * net.n)]
            for v in killed:
                if net.procs[v].poll() is None: net.kill(v)
            time.sleep(2)  # мёртвые узлы ещё в таблицах маршрутизации — lookup натыкается на них
            al = net.alive()
            for k in range(a.n):
                i = random.choice(al); t = random.choice([j for j in al if j != i])
                try: r = lookup(net, i, net.ids[t])
                except Exception as e: r = {"ok": False, "error": str(e)}
                rows.append([frac, len(killed), len(al), k, i, t, bool(r.get("ok") and r.get("found_target")),
                             r.get("rpc_count"), r.get("iterations"), r.get("duration_ms")])
        lines = []
        for frac in fracs:
            sub = [x for x in rows if x[0] == frac]
            lines.append(f"- отключено {sub[0][1]} из {net.n} ({int(frac * 100)}%): успех {sum(1 for x in sub if x[6])}/{len(sub)}; "
                         f"duration_ms {fmt(stat([x[9] for x in sub if x[6]]))}; RPC {fmt(stat([x[7] for x in sub if x[6]]))}")
        run.finish(["fail_fraction", "killed", "alive", "n", "initiator", "target", "found", "rpc_count", "iterations", "duration_ms"], rows,
                   "Часть узлов останавливается (SIGKILL), через 2 с — пока они ещё числятся в таблицах маршрутизации "
                   "и попадают в кандидаты lookup — живые узлы ищут друг друга. Отключения накопительные.\n\n" + "\n".join(lines))
    finally:
        net.stop()


# ── 06: туннель и сообщения (первое сообщение vs установленная сессия) ────────

def s06(a):
    per_pair = 3
    run = Run("06_tunnel_message", a, pairs=a.n, messages_per_pair=per_pair)
    net = Net("s06", a.nodes); rows = []
    try:
        net.converge()
        pairs = [(s, r) for s in range(net.n) for r in range(s + 1, net.n)]
        random.shuffle(pairs)
        for p, (s, r) in enumerate(pairs[:a.n]):
            if random.random() < 0.5: s, r = r, s
            for m in range(per_pair):
                res = send(net, s, r, f"s06-{p}-{m}")
                reused = res.get("tunnel_reused")
                kind = "" if reused is None else ("session" if reused else "first")
                rows.append([p, m, s, r, kind, res.get("delivered"), res.get("rtt_ms"), res.get("lookup_ms"),
                             res.get("tunnel_build_ms"), res.get("tunnel_send_ms"), res.get("rebuilt_after_failure"),
                             res.get("error", "")])
        ok = [x for x in rows if x[5]]
        first = [x for x in ok if x[4] == "first"]; sess = [x for x in ok if x[4] == "session"]
        run.finish(["pair", "msg", "sender", "receiver", "kind", "delivered", "rtt_ms", "lookup_ms", "tunnel_build_ms",
                    "tunnel_send_ms", "rebuilt", "error"], rows,
                   f"{a.n} пар (неупорядоченные пары не повторяются, туннелей между ними до первого сообщения нет), "
                   f"по {per_pair} сообщения на пару. Доставлено с APP_ACK {len(ok)}/{len(rows)}.\n"
                   f"first — туннель строился в этой отправке (lookup + TUNNEL_BUILD; у получателя — свой обратный туннель для APP_ACK); "
                   f"session — туннель уже установлен, lookup не выполняется.\n\n"
                   f"- время построения туннеля, мс: {fmt(stat([x[8] for x in first]))}\n"
                   f"- lookup получателя (first), мс: {fmt(stat([x[7] for x in first]))}\n"
                   f"- первое сообщение, RTT до APP_ACK, мс: {fmt(stat([x[6] for x in first]))}\n"
                   f"- установленная сессия, RTT до APP_ACK, мс: {fmt(stat([x[6] for x in sess]))}\n"
                   f"- установленная сессия, проход по цепочке до получателя (TUNNEL_ACK), мс: {fmt(stat([x[9] for x in sess]))}")
    finally:
        net.stop()


# ── 07: отказ активного ретранслятора ────────────────────────────────────────

def route_of(net, s, r):
    try:
        return (http(net.gui[s], "GET", "/api/route", target=net.ids[r]).get("tunnel") or {}).get("hops", [])
    except Exception:
        return []


def s07(a):
    run = Run("07_relay_failure", a, injections=a.inj)
    net = Net("s07", a.nodes); rows = []
    try:
        net.converge()
        for k in range(a.inj):
            s, r = random.sample(net.alive(), 2)
            warm = send(net, s, r, f"s07-warm-{k}")
            hops = route_of(net, s, r)
            addr = {f"127.0.0.1:{net.ports[i]}": i for i in net.alive()}
            idx = [addr[h["addr"]] for h in hops if h["addr"] in addr]
            if not warm.get("delivered") or not idx:
                rows.append([k, s, r, "", "", warm.get("delivered"), "", "", "", "", "", "no-active-tunnel"]); continue
            pos = k % len(idx); victim = idx[pos]
            net.kill(victim)
            t0 = time.monotonic(); fails = 0; res = {}
            while time.monotonic() - t0 < 60:
                res = send(net, s, r, f"s07-after-{k}-{fails}")
                if res.get("delivered"): break
                fails += 1
            rec = (time.monotonic() - t0) * 1000 if res.get("delivered") else None
            new_route = [h["addr"] for h in route_of(net, s, r)]
            avoided = f"127.0.0.1:{net.ports[victim]}" not in new_route
            rows.append([k, s, r, victim, pos, True, fails, r1(rec) if rec else "", res.get("rebuilt_after_failure"),
                         res.get("tunnel_build_ms"), avoided, "recovered" if rec else "not-recovered"])
            net.restart(victim)
            net.converge(timeout=20, stable=2)
        okr = [x for x in rows if x[11] == "recovered"]
        inj = [x for x in rows if x[3] != ""]
        run.finish(["n", "sender", "receiver", "killed_relay", "hop_index", "delivered_before", "failed_attempts", "recovery_ms",
                    "rebuilt_in_send", "rebuild_ms", "new_route_avoids_killed", "outcome"], rows,
                   f"Отправитель строит туннель (сообщение доставлено), затем relay этого туннеля отключается (SIGKILL; "
                   f"hop_index чередуется), и сообщение отправляется повторно до доставки (лимит 60 с). recovery_ms — от "
                   f"отключения до APP_ACK; включает обнаружение отказа (нет TUNNEL_ACK), lookup, построение нового туннеля "
                   f"без отказавшего relay и повтор. После инъекции relay перезапускается.\n\n"
                   f"- инъекций {len(inj)}, доставка восстановлена {len(okr)}/{len(inj)}; "
                   f"без единой неудачной попытки (перестроение внутри той же отправки): {sum(1 for x in okr if x[6] == 0)}\n"
                   f"- новый маршрут не содержит отключённый relay: {sum(1 for x in okr if x[10])}/{len(okr)}\n"
                   f"- время восстановления доставки, мс: {fmt(stat([x[7] for x in okr]))}\n"
                   f"- из них построение нового туннеля, мс: {fmt(stat([x[9] for x in okr if x[9] is not None]))}")
    finally:
        net.stop()


# ── 08: негативные криптографические и replay-тесты ──────────────────────────

def s08(a):
    run = Run("08_negative_crypto", a)
    r = subprocess.run(["cargo", "test", "--tests"], cwd=ROOT, capture_output=True, text=True)
    rows = re.findall(r"^test (\S+) \.\.\. (\w+)", r.stdout, re.M)
    pat = r"neg|bad|wrong|replay|tamper|reject|forged|expired|mismatch|foreign|stale|rollback|invalid|unknown|oversize|too_large|loop|fail|attack"
    sel = [(n, s) for n, s in rows if re.search(pat, n)]
    open(f"{run.dir}/cargo_test.txt", "w").write(r.stdout + r.stderr)
    run.finish(["test", "status"], sel,
               f"Реальный вывод `cargo test --tests`: всего тестов {len(rows)}, из них ok {sum(1 for _, s in rows if s == 'ok')}; "
               f"негативных (по имени) {len(sel)}, из них ok {sum(1 for _, s in sel if s == 'ok')}. Полный вывод — cargo_test.txt.")
    if any(s != "ok" for _, s in rows): sys.exit(1)


# ── pcap: в захвате оверлейного трафика нет открытого текста (недели 7–8) ────

def spcap(a):
    dumpcap = shutil.which("dumpcap") or shutil.which("tcpdump")
    if not dumpcap:
        sys.exit("нужен dumpcap (wireshark-cli) или tcpdump с правом захвата на lo")
    marker = f"PLAINTEXT-MARKER-{uuid.uuid4().hex}"
    run = Run("pcap_no_plaintext", a, marker=marker)
    net = Net("pcap", a.nodes)
    p2p_lo, p2p_hi = min(net.ports), max(net.ports)
    gui_lo, gui_hi = min(net.gui), max(net.gui)
    overlay = f"{run.dir}/overlay.pcapng"; control = f"{net.dir}/gui_control.pcapng"
    def cap(path, flt):
        cmd = [dumpcap, "-q", "-i", "lo", "-f", flt, "-w", path] if "dumpcap" in dumpcap else \
              [dumpcap, "-i", "lo", "-U", "-w", path, flt]
        return subprocess.Popen(cmd, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
    caps = [cap(overlay, f"tcp portrange {p2p_lo}-{p2p_hi}"), cap(control, f"tcp portrange {gui_lo}-{gui_hi}")]
    rows = []
    try:
        time.sleep(2)
        net.converge()
        for k in range(5):
            s, r = random.sample(range(net.n), 2)
            res = send(net, s, r, f"{marker} #{k}")
            rows.append(["message", k, s, r, res.get("delivered")])
            st = http(net.gui[s], "POST", "/api/store", key=f"pcap-{k}", value=f"{marker} dht #{k}")
            rows.append(["dht_store", k, s, "", st.get("ok")])
        time.sleep(1)
    finally:
        for c in caps: c.send_signal(signal.SIGINT)
        for c in caps:
            try: c.wait(5)
            except Exception: c.kill()
        net.stop()
    data = open(overlay, "rb").read(); ctrl = open(control, "rb").read()
    m = marker.encode()
    found_overlay, found_ctrl = m in data, m in ctrl
    tls = ""
    if shutil.which("tshark"):
        q = subprocess.run(["tshark", "-r", overlay, "-q", "-z", "io,phs"], capture_output=True, text=True).stdout
        v = subprocess.run(["tshark", "-r", overlay, "-Y", "tls.handshake.type == 2", "-T", "fields",
                            "-e", "tls.handshake.extensions.supported_version"], capture_output=True, text=True).stdout.split()
        names = {"0x0304": "TLS 1.3", "0x0303": "TLS 1.2"}
        tls = q + "\nСогласованная версия в ServerHello: " + ", ".join(
            f"{names.get(x, x)} ({x}) × {v.count(x)}" for x in sorted(set(v))) + "\n"
        open(f"{run.dir}/tshark_protocols.txt", "w").write(tls)
    rows.append(["check_overlay_contains_marker", "", "", "", found_overlay])
    rows.append(["control_gui_contains_marker", "", "", "", found_ctrl])
    run.finish(["what", "k", "sender", "receiver", "result"], rows,
               f"Захват `dumpcap -i lo` на портах оверлея {p2p_lo}–{p2p_hi} во время 5 сообщений и 5 DHT STORE, "
               f"содержащих маркер `{marker}`.\n\n"
               f"- маркер в трафике оверлея (overlay.pcapng, {len(data)} байт): **{'НАЙДЕН — утечка!' if found_overlay else 'не найден'}**\n"
               f"- положительный контроль: тот же маркер в HTTP-запросах к панели узла (порты {gui_lo}–{gui_hi}, "
               f"открытый localhost-HTTP): {'найден — метод поиска работает' if found_ctrl else 'НЕ найден — проверка недостоверна'}\n"
               f"- протоколы в захвате оверлея — tshark_protocols.txt")
    if found_overlay or not found_ctrl:
        sys.exit(1)


SC = {"01": s01, "02": s02, "03": s03, "04": s04, "05": s05, "06": s06, "07": s07, "08": s08, "09": s09, "pcap": spcap}

if __name__ == "__main__":
    ap = argparse.ArgumentParser()
    ap.add_argument("which", help="01..09 | pcap | all")
    ap.add_argument("--nodes", type=int, default=12, help="число узлов N")
    ap.add_argument("--n", type=int, default=30, help="наблюдений (lookup / операций / пар сообщений)")
    ap.add_argument("--runs", type=int, default=5, help="независимых запусков для холодного старта")
    ap.add_argument("--inj", type=int, default=10, help="инъекций отказа (04, 07)")
    ap.add_argument("--seed", type=int, default=1)
    a = ap.parse_args()
    subprocess.run(["cargo", "build", "--bins"], cwd=ROOT, check=True)
    todo = [k for k in SC if k != "pcap"] if a.which == "all" else [a.which]
    for k in todo:
        if k not in SC:
            sys.exit(f"неизвестный сценарий {k}; есть: {', '.join(SC)}, all")
        print(f"\n### сценарий {k}"); SC[k](a)
