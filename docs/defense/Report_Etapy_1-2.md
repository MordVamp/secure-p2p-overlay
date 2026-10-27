# Краткий отчёт по этапам 1–2

> Данные: `experiments/results/accept_pilot/` (коммит `a2b44fb`), воспроизведение: `bash scripts/run_acceptance.sh 13 30`.

## 1. Язык и библиотеки
Rust 2021, tokio (async TCP), rmp-serde (MessagePack), ed25519-dalek, sha2, serde, tracing, clap, csv/serde_json.
Ответ на Ustnie_Trebovaniya §8: скрипты и графики — на Rust/bash, Python не используется.

## 2. Формат кадра
24-байтовый заголовок: `version`(1) `type`(1) `flags`(2) `request_id`(16, UUIDv4) `payload_length`(4, BE) + payload.
`MAX_FRAME_PAYLOAD = 65536` проверяется до выделения буфера. `FrameReader` — конечный автомат (частичные/склеенные чтения,
неверная версия/тип → ошибка без остановки процесса). Типы этапов 1–2: PING, PONG, FIND_NODE_REQUEST/RESPONSE, ERROR.

## 3. Сериализация
MessagePack: компактнее JSON (бинарные NodeID/ключи без base64), есть схема-в-коде через serde; Protobuf потребовал бы кодогенерации.
JSON оставлен для экспортов (routing tables, результаты).

## 4. NodeID
`NodeID = SHA-256(0x01 ‖ ed25519_pubkey)`. Ключ хранится в `NODE_STATE_DIR/identity.key`, поэтому ID стабилен при перезапуске (T2.1).
При получении контакта NodeID пересчитывается и сравнивается (T2.4; проверка добавлена в PING/FIND_NODE/PONG/контакты ответов).
Независимые тест-векторы: `docs/test_vectors/nodeid_xor.json` (сгенерированы Python hashlib, проверяются `tests/test_vectors.rs`).

## 5. k-bucket
256 bucket по ёмкости K=3, XOR-метрика (беззнаковое 256-бит BE), порядок LRU→MRU. Полный bucket: PING старейшего контакта —
жив → новый не добавляется (T2.8), мёртв → вытесняется (T2.9). Конфликт NodeID с другим ключом отклоняется (§18.3).

## 6. Bootstrap и lookup
Star: seed без bootstrap-контактов; остальные делают PING seed и self-lookup. Итеративный lookup: α=3 параллельных FIND_NODE,
остановка при отсутствии прогресса, журнал по итерациям (опрошенные узлы), число RPC/итераций/длительность.

## 7. Результаты приёмки (N=13, K=3)
| Показатель | Значение |
|---|---|
| Холодная сходимость | 16997 мс |
| Узлов с < N−1 контактами | 13/13 (критерий ≥80%) |
| Контактов в таблице (min / median / max) | 3 / 5 / 10 |
| Lookup до отказа bootstrap | n=30, цель отсутствовала в таблице инициатора, найдено 30, через промежуточный узел 30 |
| RPC на lookup (min/median/mean/max) | 4 / 5 / 5.3 / 9 |
| Итераций (median) | 3 |
| Длительность lookup (median / max) | 6 / 11 мс |
| Lookup после остановки bootstrap | n=10, найдено 10 |
| FIND_VALUE после отказа bootstrap | успешно |

p95 приведён в `summary.json` только при n ≥ 20 (ТЗ §15.3).

## 8. Тесты
T1.1–T1.10 (`tests/test_framing.rs`), T2.1–T2.14 (`tests/test_identity.rs`, `tests/test_routing.rs`), тест-векторы (`tests/test_vectors.rs`);
логи прогонов тестов — `logs/tests/`. Автоматическая приёмка: `p2p-orchestrator acceptance`.

## 9. Ограничения
- Все узлы на одной машине (localhost); bootstrap-граф = star, сравнение со ring — этап 8.
- Транспорт пока без TLS (этап 3 — защита канала); сообщения подписаны, но не зашифрованы.
- Пилот однократный; для итоговой статистики требуется ≥5 независимых запусков на конфигурацию.
- Время сходимости включает фиксированные паузы запуска (300 мс/узел) и окна опроса 5 с.

## 10. План следующего этапа
Подключить TLS 1.3 с взаимной аутентификацией и привязкой ключа к NodeID, негативные тесты (подмена ключа, replay),
подписанные записи с sequence_number, republish и TTL-очистка.
