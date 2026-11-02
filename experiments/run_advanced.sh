#!/usr/bin/env bash
# experiments/run_advanced.sh — продвинутый уровень (ТЗ §12, §13.2, §14, §15): N=21, k=4, 3 relay, пул из 3 туннелей.
# Сценарии 01–09 (09 — star/ring/tree/multi-seed), отказ relay с переключением на резервный, стратегии выбора relay,
# передача файлов, TLS против собственного рукопожатия. Сценарии 10 (netem) и underlay (nftables) требуют root:
#   sudo -v && bash experiments/run_advanced.sh --with-root
set -euo pipefail
cd "$(dirname "$0")/.."
cargo build --bins -q
O=./target/debug/p2p-orchestrator
ADV=(--node-config config/node_config_advanced.yaml --nodes 21)
"$O" scenario all "${ADV[@]}"
"$O" scenario failover "${ADV[@]}"
"$O" scenario strategy "${ADV[@]}" --n 60
"$O" scenario files "${ADV[@]}"
"$O" scenario channel "${ADV[@]}"
if [[ "${1:-}" == "--with-root" ]]; then
  "$O" scenario 10 "${ADV[@]}"
  "$O" scenario underlay "${ADV[@]}"
fi
