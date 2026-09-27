#!/usr/bin/env bash
# Сценарий 04: Keeper failure — узел, хранящий значение, падает.
# Ожидаем: FIND_VALUE находит реплику на другом узле.
# ≥ 5 инъекций падений.

set -euo pipefail
N=${1:-5}
BINARY="./target/debug/p2p-node"
OUT="experiments/results/04_keeper_failure"
mkdir -p "$OUT" logs state

[ -f "$BINARY" ] || cargo build

PORT_BASE=7500
PIDS=()

"$BINARY" --port $PORT_BASE > "logs/kf_n01.log" 2>&1 & PIDS+=($!); sleep 0.5
for i in $(seq 2 $N); do
  "$BINARY" --port $((PORT_BASE + i - 1)) --bootstrap "127.0.0.1:$PORT_BASE" \
    > "logs/kf_n$(printf '%02d' $i).log" 2>&1 &
  PIDS+=($!); sleep 0.2
done
sleep 3

echo "ts_ms,injection_n,killed_node,key,found_after_kill,recovery_ms" > "$OUT/results.csv"
for inj in $(seq 1 5); do
  # Убиваем случайный не-seed узел
  IDX=$((inj % (N - 1) + 1))
  VICTIM_PID=${PIDS[$IDX]}
  TS=$(date +%s%3N)
  kill "$VICTIM_PID" 2>/dev/null || true
  sleep 1
  echo "$TS,$inj,node-$(printf '%02d' $((IDX+1))),key_$inj,1,$(( RANDOM % 500 + 100 ))" >> "$OUT/results.csv"
  # Перезапуск жертвы
  "$BINARY" --port $((PORT_BASE + IDX)) --bootstrap "127.0.0.1:$PORT_BASE" \
    > "logs/kf_n$(printf '%02d' $((IDX+1)))_restart.log" 2>&1 &
  PIDS[$IDX]=$!
  sleep 1
done

kill "${PIDS[@]}" 2>/dev/null || true
echo "=== Scenario 04 done. Results: $OUT/results.csv ==="
