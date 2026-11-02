#!/usr/bin/env bash
# scripts/run_acceptance.sh — одна команда: сборка → сеть из N узлов → сходимость →
# выгрузка таблиц → контрольные lookup → отказ bootstrap → повтор → сводка.
# Использование: bash scripts/run_acceptance.sh [N=13] [LOOKUPS=30]
set -euo pipefail
N=${1:-13}; L=${2:-30}
cargo build --bins
OUT="experiments/results/accept_$(date +%Y%m%d_%H%M%S)"
./target/debug/p2p-orchestrator acceptance --nodes "$N" --lookups "$L" --out "$OUT"
./target/debug/p2p-orchestrator plot "$OUT"        # графики SVG + stats.md из сохранённых CSV
# запасной вариант (matplotlib, PNG): python3 scripts/python/analyze.py "$OUT"
echo "Результаты: $OUT (summary.md, lookups_*.csv/json, routing_tables/, routing_summary.csv, stats.md, plots/*.svg); логи узлов: logs/$(basename "$OUT")/"
