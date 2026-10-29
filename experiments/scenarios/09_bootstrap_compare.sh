#!/usr/bin/env bash
# Сценарий 09 (ТЗ §15.1): Сравнение bootstrap-схем star / ring.
# Реальные процессы p2p-node, измерения — scripts/scenarios.py; результаты:
# experiments/results/09_bootstrap_compare/<timestamp>/{results.csv,summary.md,meta.json}
# Аргументы передаются драйверу: --nodes N --n OBS --runs R --inj I --seed S
set -euo pipefail
cd "$(dirname "$0")/../.."
exec python3 scripts/scenarios.py 09 "$@"
