# Эксперименты

Реальные измерения — только через `p2p-orchestrator acceptance` (см. `scripts/run_acceptance.sh`).
Результаты прогона: `experiments/results/accept_*/` (summary.md/json, lookups_*.csv/json, routing_tables/, routing_summary.csv, metrics/).
Логи узлов — `logs/accept_*/` (вне git). Закрытые ключи (`state/`) удаляются из каталога результатов.

## Сценарии 02–09
Файлы сохранены как каркасы, но **отключены** (`exit 2` в начале): прежние версии писали в CSV значения из `$RANDOM`
или константы (запрещено ТЗ §22). Сценарии 01 и `run_experiment.sh` измеряют реально. Остальные нужно переписать
на реальные метрики по мере готовности этапов (STORE-отказ хранителя, туннель, отказ relay, bootstrap star/ring).
