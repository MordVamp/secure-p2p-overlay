# Результаты приёмки этапов 1–2

N=13, k=3, коммит `a2b44fbdd9994d9653121a447ab5fe7a2365fed5`.

| Критерий | Результат |
|---|---|
| converged | ✅ |
| find_value_after_bootstrap_loss | ✅ |
| lookups_after_bootstrap_loss_ok | ✅ |
| lookups_before_ge_required_and_95pct_success_and_intermediate | ✅ |
| tables_non_degenerate_ge80pct_below_N-1 | ✅ |

Сходимость: 16997 мс. Узлов с < N-1 контактами: 13/13.
Lookup до отказа: {"duration_ms":{"max":11.0,"mean":7.3,"median":6.0,"min":5.0,"n":30,"p95":11.0},"found_target":30,"iterations":{"max":3.0,"mean":3.0,"median":3.0,"min":3.0,"n":30,"p95":3.0},"rpc_count":{"max":9.0,"mean":5.333333333333333,"median":5.0,"min":4.0,"n":30,"p95":8.0},"success_rate":1.0,"target_absent_before":30,"total_lookups":30,"used_intermediate_node":30}
Lookup после отказа bootstrap: {"duration_ms":{"max":8.0,"mean":6.2,"median":6.0,"min":5.0,"n":10},"found_target":10,"iterations":{"max":3.0,"mean":3.0,"median":3.0,"min":3.0,"n":10},"rpc_count":{"max":6.0,"mean":4.4,"median":4.0,"min":3.0,"n":10},"success_rate":1.0,"target_absent_before":10,"total_lookups":10,"used_intermediate_node":10}
