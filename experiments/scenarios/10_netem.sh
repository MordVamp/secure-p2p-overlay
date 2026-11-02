#!/usr/bin/env bash
# Сценарий 10 (продвинутый уровень): Управляемая деградация underlay через tc/netem (нужен root: сначала sudo -v).
# Драйвер — `p2p-orchestrator scenario 10`; аргументы передаются ему (--nodes --n --runs --inj --seed --channel --node-config).
# Продвинутый уровень: --node-config config/node_config_advanced.yaml --nodes 21
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo build --bins -q
exec ./target/debug/p2p-orchestrator scenario 10 "$@"
