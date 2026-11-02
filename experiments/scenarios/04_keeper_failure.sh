#!/usr/bin/env bash
# Сценарий 04 (ТЗ §15.1): Отключение хранителя и повторный FIND_VALUE (≥5 инъекций).
# Реальные процессы p2p-node, драйвер — `p2p-orchestrator scenario` (src/experiment/); результаты:
# experiments/results/04_keeper_failure/<timestamp>/{results.csv,summary.md,meta.json,plots/,node_logs/}
# Аргументы передаются драйверу: --nodes N --n OBS --runs R --inj I --seed S
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo build --bins -q
exec ./target/debug/p2p-orchestrator scenario 04 "$@"
# запасной вариант на Python: python3 scripts/python/scenarios.py 04 "$@"
