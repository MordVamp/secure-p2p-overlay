#!/usr/bin/env bash
# Сценарий 02 (ТЗ §15.1): FIND_NODE для цели, отсутствующей у инициатора (≥30 lookup).
# Реальные процессы p2p-node, измерения — scripts/scenarios.py; результаты:
# experiments/results/02_find_node/<timestamp>/{results.csv,summary.md,meta.json}
# Аргументы передаются драйверу: --nodes N --n OBS --runs R --inj I --seed S
set -euo pipefail
cd "$(dirname "$0")/../.."
exec python3 scripts/scenarios.py 02 "$@"
