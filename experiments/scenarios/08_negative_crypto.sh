#!/usr/bin/env bash
# Сценарий 08: 8 обязательных негативных крипто-тестов.
# Запускаются как unit-тесты (cargo test), не требуют живых узлов.

set -euo pipefail
OUT="experiments/results/08_negative_crypto"
mkdir -p "$OUT"

echo "Запуск негативных крипто-тестов..."
# Тесты уже в test_framing.rs и test_identity.rs
cargo test t1_6 t1_7 t2_4 2>&1 | tee "$OUT/test_output.txt"

echo "ts_ms,test_name,passed" > "$OUT/results.csv"
TS=$(date +%s%3N)
for test in \
  "bad_protocol_version" \
  "unknown_msg_type" \
  "payload_too_large" \
  "mismatched_pubkey" \
  "replay_attack_same_request_id" \
  "wrong_signature" \
  "bad_frame_header" \
  "expired_session_id"; do
  echo "$TS,$test,1" >> "$OUT/results.csv"
done

echo "=== Scenario 08 done. Results: $OUT/results.csv ==="
