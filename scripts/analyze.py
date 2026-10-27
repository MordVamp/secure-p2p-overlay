#!/usr/bin/env python3
"""scripts/analyze.py — обработка результатов приёмки и построение графиков (ТЗ §15.3).

Использование:
    python3 scripts/analyze.py [КАТАЛОГ_ПРОГОНА]
Без аргумента берётся самый свежий experiments/results/accept_*.

Входные данные (реальные, из прогона `p2p-orchestrator acceptance`):
    lookups_before.csv, lookups_after_bootstrap_loss.csv, routing_summary.csv
Выход:
    <КАТАЛОГ>/plots/*.png и <КАТАЛОГ>/stats.md (таблица со средним/медианой/p95).
Ничего не генерируется «из воздуха»: если файла нет — этап пропускается.
"""
import csv
import glob
import os
import statistics as st
import sys

import matplotlib
matplotlib.use("Agg")
import matplotlib.pyplot as plt


def latest_run():
    runs = sorted(glob.glob("experiments/results/accept_*"))
    if not runs:
        sys.exit("нет каталогов experiments/results/accept_*; сначала: bash scripts/run_acceptance.sh")
    return runs[-1]


def read_csv(path):
    if not os.path.exists(path):
        return []
    with open(path, newline="") as f:
        return list(csv.DictReader(f))


def col(rows, name, cast=float):
    return [cast(r[name]) for r in rows if r.get(name) not in (None, "")]


def pct(xs, p):
    xs = sorted(xs)
    if not xs:
        return float("nan")
    k = (len(xs) - 1) * p / 100
    f, c = int(k), min(int(k) + 1, len(xs) - 1)
    return xs[f] + (xs[c] - xs[f]) * (k - f)


def stats_row(name, xs):
    if not xs:
        return f"| {name} | 0 | — | — | — | — | — |"
    return (f"| {name} | {len(xs)} | {min(xs):.1f} | {st.mean(xs):.2f} | "
            f"{st.median(xs):.1f} | {pct(xs, 95):.1f} | {max(xs):.1f} |")


def main():
    run = sys.argv[1] if len(sys.argv) > 1 else latest_run()
    out = os.path.join(run, "plots")
    os.makedirs(out, exist_ok=True)
    before = read_csv(os.path.join(run, "lookups_before.csv"))
    after = read_csv(os.path.join(run, "lookups_after_bootstrap_loss.csv"))
    routing = read_csv(os.path.join(run, "routing_summary.csv"))
    lines = [f"# Статистика прогона `{os.path.basename(run)}`", "",
             "| Метрика | n | min | mean | median | p95 | max |", "|---|---|---|---|---|---|---|"]

    for metric, title, unit in (("duration_ms", "Длительность lookup", "мс"),
                                ("rpc_count", "Число RPC на lookup", "шт."),
                                ("iterations", "Итерации lookup", "шт.")):
        b, a = col(before, metric), col(after, metric)
        lines += [stats_row(f"{title}, до отказа bootstrap", b),
                  stats_row(f"{title}, после отказа bootstrap", a)]
        if not b and not a:
            continue
        fig, ax = plt.subplots(figsize=(6.4, 3.8))
        data, labels = [], []
        if b:
            data.append(b); labels.append(f"до отказа (n={len(b)})")
        if a:
            data.append(a); labels.append(f"после отказа (n={len(a)})")
        try:                      # matplotlib >= 3.9
            ax.boxplot(data, tick_labels=labels, showmeans=True)
        except TypeError:         # более старые версии
            ax.boxplot(data, labels=labels, showmeans=True)
        ax.set_ylabel(unit)
        ax.set_title(title)
        ax.grid(axis="y", alpha=0.3)
        fig.tight_layout()
        fig.savefig(os.path.join(out, f"{metric}.png"), dpi=130)
        plt.close(fig)

    if routing:
        ports = [r["port"] for r in routing]
        sizes = col(routing, "total_contacts", int)
        fig, ax = plt.subplots(figsize=(7.2, 3.8))
        ax.bar(ports, sizes, color="#4c78a8")
        ax.axhline(len(routing) - 1, color="#e45756", ls="--", label=f"N-1 = {len(routing) - 1}")
        ax.set_xlabel("узел (порт)")
        ax.set_ylabel("контактов в таблице")
        ax.set_title("Размер таблицы маршрутизации после сходимости")
        ax.legend()
        plt.setp(ax.get_xticklabels(), rotation=45)
        fig.tight_layout()
        fig.savefig(os.path.join(out, "routing_table_sizes.png"), dpi=130)
        plt.close(fig)
        lines += ["", f"Таблицы маршрутизации: узлов {len(sizes)}, контактов min/mean/max = "
                      f"{min(sizes)}/{st.mean(sizes):.1f}/{max(sizes)} (N-1 = {len(routing) - 1})."]

    with open(os.path.join(run, "stats.md"), "w") as f:
        f.write("\n".join(lines) + "\n")
    print("\n".join(lines))
    print(f"\nГрафики: {out}/")


if __name__ == "__main__":
    main()
