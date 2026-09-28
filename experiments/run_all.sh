#!/usr/bin/env bash
# experiments/run_all.sh — запуск всех 9 сценариев эксперимента.
# Результаты сохраняются в experiments/results/<scenario>/results.csv
#
# Использование:
#   bash experiments/run_all.sh [N_NODES]

set -euo pipefail
N=${1:-5}
SCENARIOS_DIR="$(dirname "$0")/scenarios"

echo "================================================================"
echo " P2P Overlay Network Experiment Suite"
echo " N_NODES=$N"
echo " $(date)"
echo "================================================================"

cargo build 2>&1 | tail -1
mkdir -p logs
SUITE_LOG="logs/run_all_$(date +%Y%m%d_%H%M%S).log"
echo "Suite log: $SUITE_LOG"


for script in "$SCENARIOS_DIR"/0*.sh; do
  name=$(basename "$script" .sh)
  echo ""
  echo "--- Running $name ---"
  bash "$script" "$N" 2>&1 | tee -a "$SUITE_LOG" || echo "WARNING: $name exited with error" | tee -a "$SUITE_LOG"
done

echo ""
echo "================================================================"
echo " All scenarios complete. Results in experiments/results/"
echo "================================================================"
ls -la experiments/results/ 2>/dev/null || true
