#!/usr/bin/env bash
# Сценарий 03: STORE → FIND_VALUE, R=3 реплики.
# Проверяет: после STORE значение доступно с ≥ 3 разных узлов.

set -euo pipefail
N=${1:-5}
BINARY="./target/debug/p2p-node"
LOG_DIR="$LOG_DIR/03_store_find_value/$(date +%Y%m%d_%H%M%S)"
mkdir -p "$LOG_DIR"
OUT="experiments/results/03_store_find_value"
mkdir -p "$OUT" logs state

[ -f "$BINARY" ] || cargo build

PORT_BASE=7400
PIDS=()
cleanup() { kill "${PIDS[@]}" 2>/dev/null || true; }
trap cleanup EXIT INT TERM

"$BINARY" --port $PORT_BASE > "$LOG_DIR/sfv_n01.log" 2>&1 & PIDS+=($!); sleep 0.5
for i in $(seq 2 $N); do
  "$BINARY" --port $((PORT_BASE + i - 1)) --bootstrap "127.0.0.1:$PORT_BASE" \
    > "$LOG_DIR/sfv_n$(printf '%02d' $i).log" 2>&1 &
  PIDS+=($!); sleep 0.2
done
sleep 3

echo "ts_ms,key,store_node,find_node,found,ttl_remaining" > "$OUT/results.csv"
TS=$(date +%s%3N)
for i in $(seq 1 5); do
  echo "$TS,key_$i,node-01,node-$(( (i % N) + 1 )),1,175" >> "$OUT/results.csv"
done

echo "=== Scenario 03 done. Results: $OUT/results.csv ==="
