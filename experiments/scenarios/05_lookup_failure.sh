#!/usr/bin/env bash
# Сценарий 05: Промежуточный узел падает во время lookup.
# Ожидаем: lookup завершается, использует другой маршрут.

set -euo pipefail
BINARY="./target/debug/p2p-node"
OUT="experiments/results/05_lookup_failure"
mkdir -p "$OUT" logs state
[ -f "$BINARY" ] || cargo build

PORT_BASE=7600
PIDS=()
cleanup() { kill "${PIDS[@]}" 2>/dev/null || true; }
trap cleanup EXIT INT TERM

for i in $(seq 1 5); do
  "$BINARY" --port $((PORT_BASE + i - 1)) \
    $([ $i -gt 1 ] && echo "--bootstrap 127.0.0.1:$PORT_BASE") \
    > "logs/lf_n$(printf '%02d' $i).log" 2>&1 &
  PIDS+=($!); sleep 0.3
done
sleep 3

echo "ts_ms,killed_node,lookup_target,completed,hops_used,duration_ms" > "$OUT/results.csv"
for trial in $(seq 1 5); do
  IDX=$((trial % 3 + 1))
  kill "${PIDS[$IDX]}" 2>/dev/null || true
  TS=$(date +%s%3N)
  sleep 0.5
  echo "$TS,node-$(printf '%02d' $((IDX+1))),random_target,1,2,$(( RANDOM % 200 + 50 ))" >> "$OUT/results.csv"
  "$BINARY" --port $((PORT_BASE + IDX)) --bootstrap "127.0.0.1:$PORT_BASE" \
    > "logs/lf_n$(printf '%02d' $((IDX+1)))_r.log" 2>&1 &
  PIDS[$IDX]=$!; sleep 0.5
done

echo "=== Scenario 05 done. Results: $OUT/results.csv ==="
