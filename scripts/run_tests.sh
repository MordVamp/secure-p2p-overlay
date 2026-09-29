#!/usr/bin/env bash
# scripts/run_tests.sh — запуск всех тестов с записью в logs/tests/
# Использование:
#   bash scripts/run_tests.sh            # все тесты
#   bash scripts/run_tests.sh t2_        # фильтр по имени
#   bash scripts/run_tests.sh "" --nocapture   # с выводом println!

set -euo pipefail

FILTER="${1:-}"
EXTRA_ARGS="${2:-}"
LOG_DIR="logs/tests"
mkdir -p "$LOG_DIR"
LOG_FILE="$LOG_DIR/$(date +%Y%m%d_%H%M%S)${FILTER:+_$FILTER}.log"

echo "=== P2P Overlay — Test Run ===" | tee "$LOG_FILE"
echo "Filter : '${FILTER:-<all>}'"    | tee -a "$LOG_FILE"
echo "Time   : $(date)"               | tee -a "$LOG_FILE"
echo "Log    : $LOG_FILE"             | tee -a "$LOG_FILE"
echo ""                                | tee -a "$LOG_FILE"

# Сборка
cargo build 2>&1 | tail -1 | tee -a "$LOG_FILE"

# Тесты
if [ -n "$FILTER" ]; then
    cargo test "$FILTER" -- --nocapture $EXTRA_ARGS 2>&1 | tee -a "$LOG_FILE"
else
    cargo test -- --nocapture $EXTRA_ARGS 2>&1 | tee -a "$LOG_FILE"
fi

EXIT_CODE=${PIPESTATUS[0]}

echo "" | tee -a "$LOG_FILE"
echo "=== Done. Exit code: $EXIT_CODE ===" | tee -a "$LOG_FILE"
echo "Log saved: $LOG_FILE"
exit $EXIT_CODE
