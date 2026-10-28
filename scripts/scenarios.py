#!/usr/bin/env python3
"""scripts/scenarios.py — реальные измерения сценариев 01–09 (ТЗ §22: без синтетики).

Запускает настоящие процессы `p2p-node`, управляет ими через HTTP API панели узла
и пишет CSV + summary.md в experiments/results/<сценарий>/<timestamp>/.
Использование: python3 scripts/scenarios.py <01..09|all> [--nodes N] [--n OBS]
"""
import argparse, csv, json, os, random, re, signal, statistics, subprocess, sys, time
import urllib.request, urllib.parse

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
BIN = os.path.join(ROOT, "target/debug/p2p-node")
_port = [7400]


def http(gui, method, path, **q):
    url = f"http://127.0.0.1:{gui}{path}"
    if q:
        url += "?" + urllib.parse.urlencode(q)
    req = urllib.request.Request(url, method=method, data=b"" if method == "POST" else None)
    with urllib.request.urlopen(req, timeout=20) as r:
        return json.loads(r.read())


class Net:
    """Набор живых узлов. topology: star (все к seed) | ring (i → i-1)."""

    def __init__(self, tag, n, topology="star"):
        self.dir = os.path.join(ROOT, "logs", "scenarios", f"{tag}_{int(time.time())}")
        os.makedirs(self.dir, exist_ok=True)
        self.procs, self.ports = [], []
        self.boot_at = []
        base = _port[0]; _port[0] += 2 * n + 4
        for i in range(n):
            p = base + i
            args = [BIN, "-p", str(p), "--gui-port", str(p + n + 2),
                    "--state-dir", f"{self.dir}/state{i}", "--metrics-dir", f"{self.dir}/metrics{i}"]
            if i > 0:
                b = base if topology == "star" else base + i - 1
                args += ["-b", f"127.0.0.1:{b}"]
            self.boot_at.append(time.time())
            self.procs.append(subprocess.Popen(args, stdout=open(f"{self.dir}/n{i}.log", "w"),
                                               stderr=subprocess.STDOUT, cwd=ROOT))
            self.ports.append(p)
            time.sleep(0.5 if i == 0 else 0.15)
        self.n = n
        self.gui = [p + n + 2 for p in self.ports]
        self.ids = [self._wait_id(g) for g in self.gui]

    def _wait_id(self, g):
        for _ in range(60):
            try:
                return http(g, "GET", "/api/status")["node_id"]
            except Exception:
                time.sleep(0.2)
        raise RuntimeError(f"node gui {g} не поднялся")

    def contacts(self, i):
        try:
            return http(self.gui[i], "GET", "/api/status")["routing"]["total_contacts"]
        except Exception:
            return -1

    def converge(self, timeout=60, stable=3):
        """Ждём, пока сумма контактов не меняется `stable` опросов подряд."""
        last, same, t0 = None, 0, time.time()
        while time.time() - t0 < timeout:
            s = sum(max(self.contacts(i), 0) for i in self.alive())
            same = same + 1 if s == last else 0
            last = s
            if same >= stable:
                return time.time() - t0
            time.sleep(1)
        return float("nan")

    def alive(self):
        return [i for i, p in enumerate(self.procs) if p.poll() is None]

    def kill(self, i):
        self.procs[i].kill(); self.procs[i].wait()

    def stop(self):
        for p in self.procs:
            if p.poll() is None:
                p.send_signal(signal.SIGTERM)
        for p in self.procs:
            try: p.wait(5)
            except Exception: p.kill()


def stat(xs):
    xs = [x for x in xs if x == x]
    if not xs: return {"n": 0}
    s = sorted(xs); n = len(s)
    d = {"n": n, "min": s[0], "mean": round(statistics.mean(s), 2),
         "median": statistics.median(s), "max": s[-1]}
    if n >= 20: d["p95"] = s[min(n, -(-95 * n // 100)) - 1]
    return d


def out_dir(name):
    d = os.path.join(ROOT, "experiments/results", name, time.strftime("%Y%m%d_%H%M%S"))
    os.makedirs(d, exist_ok=True)
    return d


def finish(name, d, header, rows, summary):
    with open(f"{d}/results.csv", "w", newline="") as f:
        w = csv.writer(f); w.writerow(header); w.writerows(rows)
    sha = subprocess.run(["git", "rev-parse", "HEAD"], cwd=ROOT, capture_output=True, text=True).stdout.strip()
    with open(f"{d}/summary.md", "w") as f:
        f.write(f"# {name}\n\nкоммит `{sha}`\n\n{summary}\n")
    print(f"== {name}: {d}\n{summary}")


def lookup(net, i, target):
    return http(net.gui[i], "POST", "/api/lookup", target=target)


def s01(a):
    d = out_dir("01_cold_start"); rows = []; times = []
    runs = max(a.n, 5) if a.n < 30 else 5
    for run in range(1, runs + 1):
        net = Net("s01", a.nodes)
        try:
            for i in range(net.n):
                t0 = net.boot_at[i]; ms = float("nan")
                while time.time() - t0 < 30:
                    if net.contacts(i) > 0:
                        ms = (time.time() - t0) * 1000; break
                    time.sleep(0.05)
                if i > 0: times.append(ms)
                rows.append([run, i, net.ids[i][:8], round(ms, 1) if ms == ms else "", net.contacts(i)])
        finally:
            net.stop()
    finish("01_cold_start", d, ["run", "node", "node_id", "time_to_first_contact_ms", "contacts"], rows,
           f"N={a.nodes}, запусков {runs}. Время от старта процесса до первого контакта в таблице (без seed), мс: {json.dumps(stat(times))}")


def s02(a):
    d = out_dir("02_find_node"); net = Net("s02", a.nodes); rows = []
    try:
        net.converge()
        durs, rpcs = [], []
        for k in range(max(a.n, 30)):
            i = random.randrange(net.n); t = random.choice([x for j, x in enumerate(net.ids) if j != i])
            r = lookup(net, i, t)
            ok = r.get("ok") and r.get("found_target")
            rows.append([k, i, t[:8], r.get("target_known_before"), ok, r.get("rpc_count"), r.get("iterations"), r.get("duration_ms")])
            if ok: durs.append(r["duration_ms"]); rpcs.append(r["rpc_count"])
        found = sum(1 for r in rows if r[4])
        unk = sum(1 for r in rows if r[3] is False)
        finish("02_find_node", d, ["n", "initiator", "target", "known_before", "found", "rpc_count", "iterations", "duration_ms"], rows,
               f"N={a.nodes}, lookup {len(rows)} (цель не была в таблице инициатора: {unk}), найдено {found}. duration_ms: {json.dumps(stat(durs))}; rpc: {json.dumps(stat(rpcs))}")
    finally:
        net.stop()


def s03(a):
    d = out_dir("03_store_find_value"); net = Net("s03", a.nodes); rows = []
    try:
        net.converge(); st, fd = [], []
        for k in range(max(a.n, 30)):
            w, r = random.sample(range(net.n), 2); key, val = f"s03-{k}-{random.random()}", f"v{k}"
            t0 = time.time(); s = http(net.gui[w], "POST", "/api/store", key=key, value=val)
            tw = (time.time() - t0) * 1000
            t0 = time.time(); f = http(net.gui[r], "GET", "/api/find", key=key)
            tr = (time.time() - t0) * 1000
            ok = bool(f.get("found")) and f.get("value") == val
            rows.append([k, w, r, s.get("ok"), s.get("stored_on"), round(tw, 1), ok, round(tr, 1)])
            if ok: fd.append(tr)
            if s.get("ok"): st.append(tw)
        okc = sum(1 for r in rows if r[6])
        finish("03_store_find_value", d, ["n", "writer", "reader", "store_ok", "stored_on", "store_ms", "found_ok", "find_ms"], rows,
               f"N={a.nodes}, операций {len(rows)}, значение прочитано верно {okc}. store_ms: {json.dumps(stat(st))}; find_ms: {json.dumps(stat(fd))}")
    finally:
        net.stop()


def holders(net, key):
    out = []
    for i in net.alive():
        try:
            if key in json.dumps(http(net.gui[i], "GET", "/api/status")["store"]): out.append(i)
        except Exception: pass
    return out


def s04(a):
    d = out_dir("04_keeper_failure"); net = Net("s04", a.nodes); rows = []
    try:
        net.converge(); writer = 0
        for k, kill_n in enumerate([1, 2, 3] * 4):
            key = f"s04-{k}-{random.random()}"
            http(net.gui[writer], "POST", "/api/store", key=key, value="keep")
            time.sleep(0.5)
            hs = [h for h in holders(net, key) if h != writer]
            victims = hs[:kill_n]
            for v in victims: net.kill(v)
            time.sleep(1)
            r = random.choice([i for i in net.alive() if i != writer])
            t0 = time.time(); f = http(net.gui[r], "GET", "/api/find", key=key); ms = (time.time() - t0) * 1000
            rows.append([k, len(hs), len(victims), r, bool(f.get("found")), round(ms, 1)])
            if len(net.alive()) <= max(5, net.n // 2): break
        finish("04_keeper_failure", d, ["n", "holders_excl_writer", "killed", "reader", "found_after", "find_ms"], rows,
               f"N={a.nodes}. Убивались хранители записи (кроме автора), чтение с живого узла. Попыток {len(rows)}, найдено {sum(1 for r in rows if r[4])}.")
    finally:
        net.stop()


def s05(a):
    d = out_dir("05_lookup_failure"); net = Net("s05", a.nodes); rows = []
    fracs = (0, 0.2, 0.4)
    try:
        net.converge()
        order = list(range(1, net.n)); random.shuffle(order)
        for frac in fracs:
            for v in order[:int(frac * net.n)]:
                if net.procs[v].poll() is None: net.kill(v)
            time.sleep(8)
            al = net.alive()
            for k in range(max(a.n, 30)):
                i = random.choice(al); t = random.choice([net.ids[j] for j in al if j != i])
                try: r = lookup(net, i, t)
                except Exception: r = {"ok": False}
                rows.append([frac, len(al), k, bool(r.get("ok") and r.get("found_target")), r.get("rpc_count"), r.get("duration_ms")])
        lines = []
        for frac in fracs:
            sub = [r for r in rows if r[0] == frac]
            lines.append(f"- отказов {int(frac*100)}% (живых {sub[0][1]}): успех {sum(1 for r in sub if r[3])}/{len(sub)}")
        finish("05_lookup_failure", d, ["fail_fraction", "alive", "n", "found", "rpc_count", "duration_ms"], rows,
               f"N={a.nodes}. Живые узлы ищут друг друга после остановки части процессов (ожидание перестройки 8 с).\n" + "\n".join(lines))
    finally:
        net.stop()


def s06(a):
    d = out_dir("06_tunnel_message"); net = Net("s06", a.nodes); rows = []
    try:
        net.converge(); rtts = []
        for k in range(max(a.n, 30)):
            s, r = random.sample(range(net.n), 2)
            res = http(net.gui[s], "POST", "/api/send", target=net.ids[r], text=f"m{k}")
            rows.append([k, s, r, res.get("delivered"), res.get("rtt_ms"), res.get("error", "")])
            if res.get("delivered"): rtts.append(res["rtt_ms"])
        okc = sum(1 for r in rows if r[3])
        finish("06_tunnel_message", d, ["n", "sender", "receiver", "delivered", "rtt_ms", "error"], rows,
               f"N={a.nodes}, сообщений {len(rows)}, доставлено с APP_ACK {okc}. rtt_ms: {json.dumps(stat(rtts))}")
    finally:
        net.stop()


def s07(a):
    d = out_dir("07_relay_failure"); net = Net("s07", a.nodes); rows = []
    try:
        net.converge()
        for k in range(max(a.n, 6) if a.n < 30 else 6):
            alive = net.alive()
            if len(alive) < 6: break
            s, r = random.sample(alive, 2)
            base = http(net.gui[s], "POST", "/api/send", target=net.ids[r], text="warm")
            route = http(net.gui[s], "GET", "/api/route", target=net.ids[r]).get("tunnel") or {}
            relays = [h["addr"] for h in route.get("hops", [])]
            idx = [i for i in alive if i not in (s, r) and f"127.0.0.1:{net.ports[i]}" in relays]
            if not idx:
                rows.append([k, s, r, "", base.get("delivered"), "", "", "no-relay-found"]); continue
            net.kill(idx[0]); t0 = time.time(); rec = None; fails = 0
            while time.time() - t0 < 60:
                res = http(net.gui[s], "POST", "/api/send", target=net.ids[r], text="after")
                if res.get("delivered"): rec = (time.time() - t0) * 1000; break
                fails += 1
            rows.append([k, s, r, idx[0], base.get("delivered"), fails, round(rec, 1) if rec else "", "recovered" if rec else "not-recovered"])
        rec = [r[6] for r in rows if r[6] != ""]
        finish("07_relay_failure", d, ["n", "sender", "receiver", "killed_relay", "delivered_before", "failed_attempts", "recovery_ms", "outcome"], rows,
               f"N={a.nodes}. Убивался relay активного туннеля, затем повторная отправка до успеха. Попыток {len(rows)}, восстановлено {len(rec)}. recovery_ms: {json.dumps(stat(rec))}")
    finally:
        net.stop()


def s08(a):
    d = out_dir("08_negative_crypto")
    r = subprocess.run(["cargo", "test", "--tests"], cwd=ROOT, capture_output=True, text=True)
    rows = re.findall(r"^test (\S+) \.\.\. (\w+)", r.stdout, re.M)
    pat = r"neg|bad|wrong|replay|tamper|reject|forged|expired|mismatch|foreign|stale|rollback|invalid|unknown|oversize|too_large|loop|fail|attack"
    sel = [(n, s) for n, s in rows if re.search(pat, n)]
    open(f"{d}/cargo_test.txt", "w").write(r.stdout + r.stderr)
    finish("08_negative_crypto", d, ["test", "status"], sel,
           f"Реальный вывод `cargo test`: всего тестов {len(rows)}, негативных (по имени) {len(sel)}, из них ok {sum(1 for _, s in sel if s == 'ok')}. Полный вывод — cargo_test.txt.")
    if any(s != "ok" for _, s in rows): sys.exit(1)


def s09(a):
    d = out_dir("09_bootstrap_compare"); rows = []; agg = {}
    for topo in ("star", "ring"):
        for run in range(3):
            net = Net("s09" + topo, a.nodes, topo)
            try:
                t = net.converge()
                cs = [net.contacts(i) for i in range(net.n)]
                rows.append([topo, run, round(t, 2), sum(cs), min(cs), max(cs)])
                agg.setdefault(topo, []).append((t, statistics.mean(cs)))
            finally:
                net.stop()
    lines = [f"- {k}: сходимость (с) {json.dumps(stat([x[0] for x in v]))}, среднее число контактов {round(statistics.mean(x[1] for x in v), 2)}" for k, v in agg.items()]
    finish("09_bootstrap_compare", d, ["topology", "run", "converge_s", "contacts_sum", "contacts_min", "contacts_max"], rows,
           f"N={a.nodes}. Сходимость = до стабильной суммы контактов (3 опроса по 1 с подряд, т.е. включает ≥3 с ожидания).\n" + "\n".join(lines))


SC = {"01": s01, "02": s02, "03": s03, "04": s04, "05": s05, "06": s06, "07": s07, "08": s08, "09": s09}

if __name__ == "__main__":
    ap = argparse.ArgumentParser()
    ap.add_argument("which"); ap.add_argument("--nodes", type=int, default=12); ap.add_argument("--n", type=int, default=30)
    a = ap.parse_args()
    subprocess.run(["cargo", "build", "--bins"], cwd=ROOT, check=True)
    for k in (SC if a.which == "all" else [a.which]):
        print(f"\n### сценарий {k}"); SC[k](a)
