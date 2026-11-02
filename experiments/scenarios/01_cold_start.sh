#!/usr/bin/env bash
# Сценарий 01 (ТЗ §15.1): Холодный запуск и операционная сходимость (≥5 запусков).
# Реальные процессы p2p-node, драйвер — `p2p-orchestrator scenario` (src/experiment/); результаты:
# experiments/results/01_cold_start/<timestamp>/{results.csv,summary.md,meta.json,plots/,node_logs/}
# Аргументы передаются драйверу: --nodes N --n OBS --runs R --inj I --seed S
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo build --bins -q
exec ./target/debug/p2p-orchestrator scenario 01 "$@"
# запасной вариант на Python: python3 scripts/python/scenarios.py 01 "$@"
