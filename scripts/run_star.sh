#!/usr/bin/env bash
# scripts/run_star.sh — запуск 5 узлов со Star bootstrap (§22.3 ТЗ).
# Узел node-01 — seed. Узлы 02–05 подключаются к нему.
set -e
BINARY="./target/debug/p2p-node"
[ -f "$BINARY" ] || cargo build 2>&1

PIDS=()
cleanup() { echo "Stopping nodes..."; kill "${PIDS[@]}" 2>/dev/null; }
trap cleanup EXIT INT TERM

echo "=== Star Bootstrap ==="
mkdir -p logs

# Seed node (без bootstrap)
echo "Starting seed node-01 :7001"
RUST_LOG=p2p_overlay=info "$BINARY" --config config/nodes/node-01.yaml > logs/node-01.log 2>&1 &
PIDS+=($!)
sleep 1

# Остальные узлы
for i in 02 03 04 05; do
  PORT=$((7000 + 10#$i))
  echo "Starting node-${i} :${PORT} → bootstrap 127.0.0.1:7001"
  RUST_LOG=p2p_overlay=info "$BINARY" --config config/nodes/node-${i}.yaml > logs/node-${i}.log 2>&1 &
  PIDS+=($!)
  sleep 0.5
done

echo "All nodes started. PIDs: ${PIDS[*]}"
echo "Logs: logs/"
wait
