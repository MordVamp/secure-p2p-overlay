# Результаты приёмки этапов 1–2

N=13, k=3, коммит `4b35f860eac1a86e9ed3ec05e8d6af3baab84e45`.

| Критерий | Результат |
|---|---|
| converged | ✅ |
| find_value_after_bootstrap_loss | ✅ |
| lookups_after_bootstrap_loss_ok | ✅ |
| lookups_before_ge_required_and_95pct_success_and_intermediate | ✅ |
| tables_non_degenerate_ge80pct_below_N-1 | ✅ |

Сходимость: 17011 мс. Узлов с < N-1 контактами: 13/13.
Lookup до отказа: {"duration_ms":{"max":32.0,"mean":23.9,"median":23.5,"min":19.0,"n":30,"p95":28.0},"found_target":30,"iterations":{"max":3.0,"mean":3.0,"median":3.0,"min":3.0,"n":30,"p95":3.0},"rpc_count":{"max":8.0,"mean":5.133333333333334,"median":5.5,"min":4.0,"n":30,"p95":6.0},"success_rate":1.0,"target_absent_before":30,"total_lookups":30,"used_intermediate_node":30}
Lookup после отказа bootstrap: {"duration_ms":{"max":38.0,"mean":25.3,"median":25.0,"min":19.0,"n":10},"found_target":10,"iterations":{"max":3.0,"mean":3.0,"median":3.0,"min":3.0,"n":10},"rpc_count":{"max":7.0,"mean":4.9,"median":5.0,"min":4.0,"n":10},"success_rate":1.0,"target_absent_before":10,"total_lookups":10,"used_intermediate_node":10}
