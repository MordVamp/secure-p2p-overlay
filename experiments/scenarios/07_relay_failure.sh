#!/usr/bin/env bash
# Сценарий 07 (ТЗ §15.1): Отключение активного ретранслятора и восстановление доставки (≥5 инъекций).
# Реальные процессы p2p-node, драйвер — `p2p-orchestrator scenario` (src/experiment/); результаты:
# experiments/results/07_relay_failure/<timestamp>/{results.csv,summary.md,meta.json,plots/,node_logs/}
# Аргументы передаются драйверу: --nodes N --n OBS --runs R --inj I --seed S
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo build --bins -q
exec ./target/debug/p2p-orchestrator scenario 07 "$@"
# запасной вариант на Python: python3 scripts/python/scenarios.py 07 "$@"
