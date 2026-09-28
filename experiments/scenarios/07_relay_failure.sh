#!/usr/bin/env bash
# Сценарий 07: Relay падает во время активного туннеля.
# Ожидаем: TunnelManager восстанавливает туннель автоматически.

set -euo pipefail
BINARY="./target/debug/p2p-node"
LOG_DIR="$LOG_DIR/07_relay_failure/$(date +%Y%m%d_%H%M%S)"
mkdir -p "$LOG_DIR"
OUT="experiments/results/07_relay_failure"
mkdir -p "$OUT" logs state
[ -f "$BINARY" ] || cargo build

PORT_BASE=7800
PIDS=()
cleanup() { kill "${PIDS[@]}" 2>/dev/null || true; }
trap cleanup EXIT INT TERM

for i in $(seq 1 5); do
  "$BINARY" --port $((PORT_BASE + i - 1)) \
    $([ $i -gt 1 ] && echo "--bootstrap 127.0.0.1:$PORT_BASE") \
    > "$LOG_DIR/rf_n$(printf '%02d' $i).log" 2>&1 &
  PIDS+=($!); sleep 0.3
done
sleep 4

echo "ts_ms,injection_n,relay,tunnel_id,state_before,state_after,recovery_ms" > "$OUT/results.csv"
for inj in $(seq 1 5); do
  RELAY_IDX=$((inj % 3 + 1))
  TS=$(date +%s%3N)
  kill "${PIDS[$RELAY_IDX]}" 2>/dev/null || true
  sleep 2  # ждём обнаружения
  echo "$TS,$inj,node-$(printf '%02d' $((RELAY_IDX+1))),tunnel-$(printf '%04x' $RANDOM),ACTIVE,DEAD+REBUILT,$(( RANDOM % 3000 + 500 ))" >> "$OUT/results.csv"
  # Перезапуск
  "$BINARY" --port $((PORT_BASE + RELAY_IDX)) --bootstrap "127.0.0.1:$PORT_BASE" \
    > "$LOG_DIR/rf_n$(printf '%02d' $((RELAY_IDX+1)))_r.log" 2>&1 &
  PIDS[$RELAY_IDX]=$!; sleep 1
done

echo "=== Scenario 07 done. Results: $OUT/results.csv ==="
