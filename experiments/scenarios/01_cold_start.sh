#!/usr/bin/env bash
# Сценарий 01 (ТЗ §15.1): Холодный запуск и операционная сходимость (≥5 запусков).
# Реальные процессы p2p-node, измерения — scripts/scenarios.py; результаты:
# experiments/results/01_cold_start/<timestamp>/{results.csv,summary.md,meta.json}
# Аргументы передаются драйверу: --nodes N --n OBS --runs R --inj I --seed S
set -euo pipefail
cd "$(dirname "$0")/../.."
exec python3 scripts/scenarios.py 01 "$@"
