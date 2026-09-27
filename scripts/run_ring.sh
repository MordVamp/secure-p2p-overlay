#!/usr/bin/env bash
# scripts/run_ring.sh — запуск 5 узлов с Ring bootstrap (§22.3 ТЗ).
set -e
BINARY="./target/debug/p2p-node"
[ -f "$BINARY" ] || cargo build 2>&1

PIDS=()
cleanup() { kill "${PIDS[@]}" 2>/dev/null; }
trap cleanup EXIT INT TERM

echo "=== Ring Bootstrap ==="
mkdir -p logs state

# node-01: seed (нет bootstrap)
RUST_LOG=p2p_overlay=info "$BINARY" --config config/nodes/node-01.yaml > logs/node-01.log 2>&1 &
PIDS+=($!); sleep 1

# node-02 → node-01
RUST_LOG=p2p_overlay=info "$BINARY" --config config/nodes/node-02.yaml --bootstrap 127.0.0.1:7001 > logs/node-02.log 2>&1 &
PIDS+=($!); sleep 0.5

# node-03 → node-02
RUST_LOG=p2p_overlay=info "$BINARY" --config config/nodes/node-03.yaml --bootstrap 127.0.0.1:7002 > logs/node-03.log 2>&1 &
PIDS+=($!); sleep 0.5

# node-04 → node-03
RUST_LOG=p2p_overlay=info "$BINARY" --config config/nodes/node-04.yaml --bootstrap 127.0.0.1:7003 > logs/node-04.log 2>&1 &
PIDS+=($!); sleep 0.5

# node-05 → node-04
RUST_LOG=p2p_overlay=info "$BINARY" --config config/nodes/node-05.yaml --bootstrap 127.0.0.1:7004 > logs/node-05.log 2>&1 &
PIDS+=($!); sleep 0.5

echo "Ring topology started. PIDs: ${PIDS[*]}"
wait
