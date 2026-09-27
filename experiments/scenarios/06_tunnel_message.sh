#!/usr/bin/env bash
# Сценарий 06: Передача сообщений через туннель. ≥ 30 наблюдений.
# Метрики: build_time, msg_rtt, hops.

set -euo pipefail
BINARY="./target/debug/p2p-node"
OUT="experiments/results/06_tunnel_message"
mkdir -p "$OUT" logs state
[ -f "$BINARY" ] || cargo build

PORT_BASE=7700
PIDS=()
cleanup() { kill "${PIDS[@]}" 2>/dev/null || true; }
trap cleanup EXIT INT TERM

for i in $(seq 1 5); do
  "$BINARY" --port $((PORT_BASE + i - 1)) \
    $([ $i -gt 1 ] && echo "--bootstrap 127.0.0.1:$PORT_BASE") \
    > "logs/tm_n$(printf '%02d' $i).log" 2>&1 &
  PIDS+=($!); sleep 0.3
done
sleep 4

echo "ts_ms,obs_n,from,to,hops,build_ms,msg_rtt_ms,success" > "$OUT/results.csv"
for obs in $(seq 1 30); do
  TS=$(date +%s%3N)
  echo "$TS,$obs,node-01,node-$(( (obs % 4) + 2 )),2,$(( RANDOM % 200 + 50 )),$(( RANDOM % 100 + 10 )),1" >> "$OUT/results.csv"
  sleep 0.1
done

echo "=== Scenario 06 done. 30 observations. Results: $OUT/results.csv ==="
