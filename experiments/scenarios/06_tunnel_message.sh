#!/usr/bin/env bash
# Сценарий 06 (ТЗ §15.1): Построение туннеля и сообщения: первое vs установленная сессия.
# Реальные процессы p2p-node, измерения — scripts/scenarios.py; результаты:
# experiments/results/06_tunnel_message/<timestamp>/{results.csv,summary.md,meta.json}
# Аргументы передаются драйверу: --nodes N --n OBS --runs R --inj I --seed S
set -euo pipefail
cd "$(dirname "$0")/../.."
exec python3 scripts/scenarios.py 06 "$@"
