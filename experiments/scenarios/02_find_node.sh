#!/usr/bin/env bash
# Сценарий 02 (ТЗ §15.1): FIND_NODE для цели, отсутствующей у инициатора (≥30 lookup).
# Реальные процессы p2p-node, драйвер — `p2p-orchestrator scenario` (src/experiment/); результаты:
# experiments/results/02_find_node/<timestamp>/{results.csv,summary.md,meta.json,plots/,node_logs/}
# Аргументы передаются драйверу: --nodes N --n OBS --runs R --inj I --seed S
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo build --bins -q
exec ./target/debug/p2p-orchestrator scenario 02 "$@"
# запасной вариант на Python: python3 scripts/python/scenarios.py 02 "$@"
