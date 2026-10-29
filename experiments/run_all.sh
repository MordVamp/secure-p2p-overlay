#!/usr/bin/env bash
# experiments/run_all.sh — все сценарии 01–09 (ТЗ §15.1) на реальных процессах.
# Результаты: experiments/results/<сценарий>/<timestamp>/; логи узлов — logs/scenarios/ (вне git).
#
# Использование:
#   bash experiments/run_all.sh [--nodes N] [--n OBS] [--runs R] [--inj I] [--seed S]
# Проверка отсутствия открытого текста в трафике (нужен dumpcap/tcpdump с правом захвата на lo):
#   python3 scripts/scenarios.py pcap
set -euo pipefail
cd "$(dirname "$0")/.."
exec python3 scripts/scenarios.py all "$@"
