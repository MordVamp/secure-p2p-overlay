#!/usr/bin/env bash
# scripts/run_ring.sh — Запуск 5 узлов, Ring bootstrap.
# Каждый узел подключается к предыдущему.
# Логи → logs/ring/

set -euo pipefail

N=${1:-5}
BINARY="./target/debug/p2p-node"
LOG_DIR="logs/ring/$(date +%Y%m%d_%H%M%S)"
mkdir -p "$LOG_DIR" state

[ -f "$BINARY" ] || { echo "Building..."; cargo build; }

echo "=== Ring bootstrap: N=$N ==="
echo "Logs: $LOG_DIR"

PIDS=()
cleanup() {
    echo ""
    echo "Stopping nodes..."
    kill "${PIDS[@]}" 2>/dev/null || true
    echo "Logs saved in $LOG_DIR"
}
trap cleanup EXIT INT TERM

PORT_BASE=7101

for i in $(seq 1 $N); do
    PORT=$((PORT_BASE + i - 1))
    NODE="node-$(printf '%02d' $i)"
    mkdir -p "state/$NODE"
    if [ $i -eq 1 ]; then
        RUST_LOG=p2p_overlay=info "$BINARY" \
            --port "$PORT" \
            > "$LOG_DIR/$NODE.log" 2>&1 &
    else
        PREV_PORT=$((PORT_BASE + i - 2))
        RUST_LOG=p2p_overlay=info "$BINARY" \
            --port "$PORT" \
            --bootstrap "127.0.0.1:$PREV_PORT" \
            > "$LOG_DIR/$NODE.log" 2>&1 &
    fi
    PIDS+=($!)
    echo "  $NODE started port=$PORT pid=${PIDS[-1]}"
    sleep 0.4
done

echo ""
echo "All $N nodes running. Press Ctrl+C to stop."
echo "Tail logs: tail -f $LOG_DIR/*.log"
wait
