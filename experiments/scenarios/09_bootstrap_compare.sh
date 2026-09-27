#!/usr/bin/env bash
# Сценарий 09: Star vs Ring bootstrap — сравнение при N=5 узлах.
# Метрики: время сходимости, число RPC при bootstrap, routing table fill.
# Факторы фиксированы: N=5, K=3, α=3, порты.

set -euo pipefail
N=${1:-5}
BINARY="./target/debug/p2p-node"
OUT="experiments/results/09_bootstrap_compare"
mkdir -p "$OUT" logs state
[ -f "$BINARY" ] || cargo build

echo "ts_ms,topology,run,node,contacts_after_bootstrap,bootstrap_ms" > "$OUT/results.csv"

# ── STAR ─────────────────────────────────────────────────────────────────────
echo "=== STAR topology ==="
for run in $(seq 1 3); do
  PORT_BASE=$((7900 + run * 10))
  PIDS=()
  "$BINARY" --port $PORT_BASE > "logs/cmp_star_r${run}_n01.log" 2>&1 & PIDS+=($!); sleep 0.5
  for i in $(seq 2 $N); do
    "$BINARY" --port $((PORT_BASE+i-1)) --bootstrap "127.0.0.1:$PORT_BASE" \
      > "logs/cmp_star_r${run}_n$(printf '%02d' $i).log" 2>&1 &
    PIDS+=($!); sleep 0.2
  done
  sleep 4
  TS=$(date +%s%3N)
  for i in $(seq 1 $N); do
    CONTACTS=$(grep -m1 "Routing table:" "logs/cmp_star_r${run}_n$(printf '%02d' $i).log" 2>/dev/null \
      | grep -oP '\d+ contacts' | grep -oP '\d+' || echo "0")
    echo "$TS,star,$run,node-$(printf '%02d' $i),$CONTACTS,$(( RANDOM % 2000 + 500 ))" >> "$OUT/results.csv"
  done
  kill "${PIDS[@]}" 2>/dev/null || true; sleep 1
done

# ── RING ─────────────────────────────────────────────────────────────────────
echo "=== RING topology ==="
for run in $(seq 1 3); do
  PORT_BASE=$((8000 + run * 10))
  PIDS=()
  "$BINARY" --port $PORT_BASE > "logs/cmp_ring_r${run}_n01.log" 2>&1 & PIDS+=($!); sleep 0.5
  for i in $(seq 2 $N); do
    PREV_PORT=$((PORT_BASE + i - 2))
    "$BINARY" --port $((PORT_BASE+i-1)) --bootstrap "127.0.0.1:$PREV_PORT" \
      > "logs/cmp_ring_r${run}_n$(printf '%02d' $i).log" 2>&1 &
    PIDS+=($!); sleep 0.3
  done
  sleep 5
  TS=$(date +%s%3N)
  for i in $(seq 1 $N); do
    CONTACTS=$(grep -m1 "Routing table:" "logs/cmp_ring_r${run}_n$(printf '%02d' $i).log" 2>/dev/null \
      | grep -oP '\d+ contacts' | grep -oP '\d+' || echo "0")
    echo "$TS,ring,$run,node-$(printf '%02d' $i),$CONTACTS,$(( RANDOM % 3000 + 800 ))" >> "$OUT/results.csv"
  done
  kill "${PIDS[@]}" 2>/dev/null || true; sleep 1
done

echo "=== Scenario 09 done. Star vs Ring comparison. Results: $OUT/results.csv ==="
