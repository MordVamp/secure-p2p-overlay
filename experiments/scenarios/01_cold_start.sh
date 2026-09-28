#!/usr/bin/env bash
# Сценарий 01: Cold start — время от запуска до сходимости bootstrap.
# Метрика: время до состояния "routing table непуста" у каждого узла.
# ≥ 5 запусков, каждый логируется в experiments/results/01_cold_start/

set -euo pipefail
N=${1:-5}
RUNS=${2:-5}
BINARY="./target/debug/p2p-node"
LOG_DIR="$LOG_DIR/01_cold_start/$(date +%Y%m%d_%H%M%S)"
mkdir -p "$LOG_DIR"
OUT="experiments/results/01_cold_start"
mkdir -p "$OUT" logs state

[ -f "$BINARY" ] || cargo build

echo "ts_ms,run,node_id,bootstrap_ms,routing_contacts" > "$OUT/results.csv"

for run in $(seq 1 $RUNS); do
  echo "=== Run $run/$RUNS ==="
  PIDS=()
  PORT_BASE=$((7200 + run * 10))

  # Seed
  RUST_LOG=p2p_overlay=info "$BINARY" --port $PORT_BASE \
    > "$LOG_DIR/cs_run${run}_n01.log" 2>&1 &
  PIDS+=($!); sleep 0.5

  for i in $(seq 2 $N); do
    PORT=$((PORT_BASE + i - 1))
    RUST_LOG=p2p_overlay=info "$BINARY" \
      --port "$PORT" --bootstrap "127.0.0.1:$PORT_BASE" \
      > "$LOG_DIR/cs_run${run}_n$(printf '%02d' $i).log" 2>&1 &
    PIDS+=($!); sleep 0.2
  done

  sleep 4  # ждём bootstrap
  kill "${PIDS[@]}" 2>/dev/null || true

  # Парсим логи: ищем "Bootstrap done" или "Routing table: N contacts"
  for i in $(seq 1 $N); do
    LOG="$LOG_DIR/cs_run${run}_n$(printf '%02d' $i).log"
    TS=$(grep -m1 "Routing table:" "$LOG" 2>/dev/null | grep -oP '^\d+' || echo "0")
    CONTACTS=$(grep -m1 "Routing table:" "$LOG" 2>/dev/null | grep -oP '\d+ contacts' | grep -oP '\d+' || echo "0")
    echo "$(date +%s%3N),$run,node-$(printf '%02d' $i),0,$CONTACTS" >> "$OUT/results.csv"
  done
  sleep 1
done

echo "=== Scenario 01 done. Results: $OUT/results.csv ==="
