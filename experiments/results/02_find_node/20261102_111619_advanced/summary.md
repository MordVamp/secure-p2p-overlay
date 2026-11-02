# 02_find_node

коммит `6726459277fb79ae3b5cfc0491d234fd48ac98a6` + незакоммиченные изменения (diff sha256 `45a3311ddd6a`), N=21, k=4, α=3, R=3, relay=3, канал=tls, bootstrap=star, seed=1, конфиг `config/node_config_advanced.yaml`; драйвер `p2p-orchestrator scenario`

Lookup 30; цель отсутствовала в таблице инициатора (по данным узла): 30/30; найдено 30/30 (100%).

- duration_ms: n=30, min 24.0, медиана 35.5, среднее 35.9, max 50.0, p95 47.0
- RPC на lookup: n=30, min 6.0, медиана 8.0, среднее 8.0, max 10.0, p95 9.0
- итераций: n=30, min 3.0, медиана 3.0, среднее 3.0, max 4.0, p95 3.0

## Графики (из results.csv)

![Длительность lookup, цель вне таблицы инициатора](plots/duration_ms.svg)

![Число RPC на lookup](plots/rpc_count.svg)

![Число итераций lookup](plots/iterations.svg)

