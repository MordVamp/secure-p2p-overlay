#!/usr/bin/env bash
# Сценарий files (продвинутый уровень): Передача файлов: goodput и доля служебного трафика.
# Драйвер — `p2p-orchestrator scenario files`; аргументы передаются ему (--nodes --n --runs --inj --seed --channel --node-config).
# Продвинутый уровень: --node-config config/node_config_advanced.yaml --nodes 21
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo build --bins -q
exec ./target/debug/p2p-orchestrator scenario files "$@"
