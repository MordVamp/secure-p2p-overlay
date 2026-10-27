# Эксперименты

Реальные измерения — только через `p2p-orchestrator acceptance` (см. `scripts/run_acceptance.sh`).
Результаты каждого прогона: `experiments/results/accept_*/` (summary.md/json, lookups_*.csv/json,
routing_tables/, routing_summary.csv, metrics/, logs/). Закрытые ключи (`state/`) удаляются из каталога.

> Прежние сценарии `02–09` записывали в CSV случайные/константные значения (`$RANDOM`) и удалены:
> ТЗ §22 запрещает данные, не полученные из измерений. Они будут заново реализованы поверх оркестратора
> (этапы 3+: STORE-отказ хранителя, туннель, отказ relay, bootstrap star/ring).
