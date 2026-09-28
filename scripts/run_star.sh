#!/usr/bin/env bash
# scripts/run_star.sh — Запуск 5 узлов, Star bootstrap (§22.3 ТЗ).
# Узел node-01 — seed. Узлы 02-05 подключаются к нему.
# Логи → logs/star/

set -euo pipefail

N=${1:-5}
BINARY="./target/debug/p2p-node"
LOG_DIR="logs/star/$(date +%Y%m%d_%H%M%S)"
mkdir -p "$LOG_DIR" state

[ -f "$BINARY" ] || { echo "Building..."; cargo build; }

echo "=== Star bootstrap: N=$N ==="
echo "Logs: $LOG_DIR"

PIDS=()
cleanup() {
    echo ""
    echo "Stopping nodes..."
    kill "${PIDS[@]}" 2>/dev/null || true
    echo "Logs saved in $LOG_DIR"
}
trap cleanup EXIT INT TERM

PORT_BASE=7001

# Seed node-01
mkdir -p "state/node-01"
RUST_LOG=p2p_overlay=info "$BINARY" \
    --config config/nodes/node-01.yaml \
    --port $PORT_BASE \
    > "$LOG_DIR/node-01.log" 2>&1 &
PIDS+=($!)
echo "  node-01 started (seed) pid=${PIDS[-1]}"
sleep 0.8

# Остальные узлы → bootstrap к node-01
for i in $(seq 2 $N); do
    PORT=$((PORT_BASE + i - 1))
    NODE="node-$(printf '%02d' $i)"
    mkdir -p "state/$NODE"
    RUST_LOG=p2p_overlay=info "$BINARY" \
        --port "$PORT" \
        --bootstrap "127.0.0.1:$PORT_BASE" \
        > "$LOG_DIR/$NODE.log" 2>&1 &
    PIDS+=($!)
    echo "  $NODE started port=$PORT pid=${PIDS[-1]}"
    sleep 0.3
done

echo ""
echo "All $N nodes running. Press Ctrl+C to stop."
echo "Tail logs: tail -f $LOG_DIR/*.log"
wait
