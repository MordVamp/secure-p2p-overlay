#!/usr/bin/env bash
# experiments/run_all.sh — все сценарии 01–09 (ТЗ §15.1) на реальных процессах + проверка pcap.
# Драйвер — `p2p-orchestrator scenario` (Rust, src/experiment/); по умолчанию N=15.
# Результаты: experiments/results/<сценарий>/<время>/; рабочие каталоги узлов — logs/scenarios/ (вне git).
#
# Использование:
#   bash experiments/run_all.sh [--nodes N] [--n OBS] [--runs R] [--inj I] [--seed S]
# Запасной вариант на Python: python3 scripts/python/scenarios.py all [те же флаги]
set -euo pipefail
cd "$(dirname "$0")/.."
cargo build --bins -q
O=./target/debug/p2p-orchestrator
"$O" scenario all "$@"
# pcap: нужен dumpcap (wireshark-cli) или tcpdump с правом захвата на lo
if command -v dumpcap >/dev/null || command -v tcpdump >/dev/null; then
  "$O" scenario pcap "$@"
else
  echo "pcap пропущен: нет dumpcap/tcpdump" >&2
fi
