#!/usr/bin/env bash
# Сценарий 08 (ТЗ §15.1): Негативные криптографические и replay-тесты (cargo test).
# Реальные процессы p2p-node, измерения — scripts/scenarios.py; результаты:
# experiments/results/08_negative_crypto/<timestamp>/{results.csv,summary.md,meta.json}
# Аргументы передаются драйверу: --nodes N --n OBS --runs R --inj I --seed S
set -euo pipefail
cd "$(dirname "$0")/../.."
exec python3 scripts/scenarios.py 08 "$@"
