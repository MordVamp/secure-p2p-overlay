#!/usr/bin/env bash
# experiments/scenarios/run_experiment.sh
# Воспроизводимый эксперимент: запуск N узлов, 30+ lookup, сбор метрик.
#
# Использование:
#   bash experiments/scenarios/run_experiment.sh [N_NODES] [N_LOOKUPS]
#
# Данные сохраняются в experiments/results/YYYY-MM-DD_HH-MM/

set -euo pipefail

N_NODES=${1:-5}
N_LOOKUPS=${2:-30}
BINARY="./target/debug/p2p-node"
LOG_DIR="$LOG_DIR/run_experiment/$(date +%Y%m%d_%H%M%S)"
mkdir -p "$LOG_DIR"
RESULTS_DIR="experiments/results/$(date +%Y-%m-%d_%H-%M)"

mkdir -p "$RESULTS_DIR" logs state

[ -f "$BINARY" ] || { echo "Building..."; cargo build; }

echo "=== Experiment: N=$N_NODES, lookups=$N_LOOKUPS ==="
echo "Results: $RESULTS_DIR"

PIDS=()
cleanup() {
    echo "Stopping nodes..."
    kill "${PIDS[@]}" 2>/dev/null || true
    # Копируем метрики в results
    cp -r metrics/* "$RESULTS_DIR"/ 2>/dev/null || true
    echo "Metrics saved to $RESULTS_DIR"
}
trap cleanup EXIT INT TERM

# Запускаем seed-узел (node-01)
PORT_BASE=7100
mkdir -p "state/exp-node-01" "metrics/exp-node-01"
RUST_LOG=p2p_overlay=info "$BINARY" \
    --config config/nodes/node-01.yaml \
    --port $PORT_BASE \
    > "$LOG_DIR/exp-node-01.log" 2>&1 &
PIDS+=($!)
sleep 1

# Запускаем остальные узлы
for i in $(seq 2 $N_NODES); do
    PORT=$((PORT_BASE + i - 1))
    NODE="exp-node-$(printf '%02d' $i)"
    mkdir -p "state/$NODE" "metrics/$NODE"
    RUST_LOG=p2p_overlay=info "$BINARY" \
        --port "$PORT" \
        --bootstrap "127.0.0.1:$PORT_BASE" \
        > "$LOG_DIR/$NODE.log" 2>&1 &
    PIDS+=($!)
    sleep 0.3
done

echo "All $N_NODES nodes started. Waiting 5s for bootstrap..."
sleep 5

echo "Nodes running. PIDs: ${PIDS[*]}"
echo "Log files: logs/"
echo ""
echo "Press Ctrl+C to stop and collect metrics."
wait
