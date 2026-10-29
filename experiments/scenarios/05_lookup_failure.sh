#!/usr/bin/env bash
# Сценарий 05 (ТЗ §15.1): Отключение промежуточных кандидатов lookup.
# Реальные процессы p2p-node, измерения — scripts/scenarios.py; результаты:
# experiments/results/05_lookup_failure/<timestamp>/{results.csv,summary.md,meta.json}
# Аргументы передаются драйверу: --nodes N --n OBS --runs R --inj I --seed S
set -euo pipefail
cd "$(dirname "$0")/../.."
exec python3 scripts/scenarios.py 05 "$@"
