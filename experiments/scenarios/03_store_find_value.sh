#!/usr/bin/env bash
# Сценарий 03 (ТЗ §15.1): STORE/FIND_VALUE с тремя репликами.
# Реальные процессы p2p-node, измерения — scripts/scenarios.py; результаты:
# experiments/results/03_store_find_value/<timestamp>/{results.csv,summary.md,meta.json}
# Аргументы передаются драйверу: --nodes N --n OBS --runs R --inj I --seed S
set -euo pipefail
cd "$(dirname "$0")/../.."
exec python3 scripts/scenarios.py 03 "$@"
