#!/usr/bin/env bash
# Сценарий underlay (продвинутый уровень): Ограниченная IP-достижимость: группы на разных loopback-подсетях, nftables (нужен root: sudo -v).
# Драйвер — `p2p-orchestrator scenario underlay`; аргументы передаются ему (--nodes --n --runs --inj --seed --channel --node-config).
# Продвинутый уровень: --node-config config/node_config_advanced.yaml --nodes 21
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo build --bins -q
exec ./target/debug/p2p-orchestrator scenario underlay "$@"
