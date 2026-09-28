# Проверка выполнения ТЗ этапов 1–2

> **Источник:** [TZ_Etapy_1-2.md](TZ_Etapy_1-2.md)  
> **Навигация:** [docs/README.md](../README.md) · [CROSSLINKS](../dev/CROSSLINKS.md)  
> **Дата проверки:** 2026-09-28

---

## ✅ Выполнено

### §3–4 Структура проекта
| Модуль | Требование | Статус |
|--------|-----------|--------|
| `transport/` | TCP, кадрирование | ✅ `src/transport/` |
| `protocol/` | константы, кодеки | ✅ `src/protocol/` |
| `identity/` | ключи, NodeID | ✅ `src/identity/` |
| `routing/` | XOR, k-bucket, RoutingTable | ✅ `src/dht/routing_table.rs` |
| `rpc/` | обработчики PING/FIND_NODE | ✅ `src/rpc/` |
| `node/` | запуск, bootstrap, координация | ✅ `src/node/` |
| `tests/` | unit + integration | ✅ `tests/` |
| `scripts/` | запуск стенда | ✅ `scripts/` |

### §5 Конфигурация (все 12 параметров)
| Параметр | Статус |
|----------|--------|
| `NODE_STATE_DIR` → `state_dir` | ✅ |
| `LISTEN_HOST` | ✅ |
| `LISTEN_PORT` | ✅ |
| `BOOTSTRAP_PEERS` | ✅ |
| `NODE_ID_BITS = 256` | ✅ |
| `K_BUCKET_SIZE` (3/4) | ✅ |
| `ALPHA = 3` | ✅ |
| `CONNECT_TIMEOUT_MS` | ✅ |
| `READ_TIMEOUT_MS` | ✅ |
| `PING_TIMEOUT_MS` | ✅ |
| `MAX_FRAME_PAYLOAD = 65536` | ✅ |
| `PROTOCOL_VERSION = 1` | ✅ |
| `LOG_LEVEL` | ✅ |
| Без изменения кода при правке K/ALPHA | ✅ YAML-конфиг |

### §6 TCP-транспорт
| Требование | Статус |
|-----------|--------|
| Нет предположения «одно read = один кадр» | ✅ `FrameReader` буферизует `BytesMut` |
| MAX_FRAME_PAYLOAD проверяется ДО выделения памяти | ✅ строка 194 `framing.rs` |
| Тайм-аут RPC | ✅ `tokio::time::timeout` |
| Ошибка одного соединения не убивает процесс | ✅ `tokio::spawn` per connection |
| Нет небезопасной сериализации | ✅ MessagePack через `rmp-serde` |

### §7 Формат кадра
| Поле | Статус |
|------|--------|
| `version` (1B) | ✅ |
| `type` (1B) | ✅ |
| `flags` (2B, BE) | ✅ |
| `request_id` (16B UUID, CSPRNG) | ✅ |
| `payload_length` (4B BE) | ✅ |
| `payload` (var) | ✅ |
| Заголовок ровно 24 байта | ✅ `HEADER_SIZE = 24` |
| Ответ содержит тот же `request_id` | ✅ `Frame::new_response()` |

### §8 Типы сообщений
| Код | Имя | Статус |
|-----|-----|--------|
| `0x01` PING | ✅ |
| `0x02` PONG | ✅ |
| `0x03` FIND_NODE_REQUEST | ✅ |
| `0x04` FIND_NODE_RESPONSE | ✅ |
| `0x7F` ERROR | ✅ (исправлено с 0xFF) |

### §9 Сериализация
| Требование | Статус |
|-----------|--------|
| Формат выбран: MessagePack | ✅ |
| PingPayload: `{sender, timestamp_ms}` | ✅ |
| PongPayload: `{responder, ping_timestamp_ms, responder_timestamp_ms}` | ✅ |
| FindNodeRequest: `{sender, target_node_id}` | ✅ |
| FindNodeResponse: `{responder, target_node_id, contacts}` | ✅ |
| ErrorPayload: `{code, description}` | ✅ |
| Примеры payload в PROTOCOL.md | ⚠️ Схемы есть, примеры-значения отсутствуют |

### §11 Диспетчер
| Интерфейс | Статус |
|-----------|--------|
| `encode_frame` | ✅ `Frame::encode()` |
| `decode_frame → Frame | NeedMoreData | ProtocolError` | ✅ `FrameReader::feed()` → `ReadItem` |
| `dispatch(frame) → Response | NoResponse` | ✅ `RpcHandler::handle()` |
| `send_request(contact, msg, timeout) → Response | RpcError` | ✅ `RpcClient::ping()` / `find_node()` |
| Обработчики не читают байты из TCP сами | ✅ |

### §14 NodeID
| Требование | Статус |
|-----------|--------|
| NodeID = SHA-256(0x01 ‖ pubkey) | ✅ |
| Детерминированность | ✅ T2.3 |
| Загрузка при рестарте → тот же ID | ✅ T2.1 |
| Несоответствие NodeID/pubkey → Err | ✅ T2.4 |

### §16 Формат Contact
| Поле | Статус |
|------|--------|
| `node_id: bytes[32]` | ✅ |
| `identity_algorithm: string` | ✅ "ed25519" |
| `identity_public_key: bytes` | ✅ |
| `host: string` | ✅ |
| `port: uint16` | ✅ |
| `last_seen_ms: uint64` | ✅ |
| `last_verified_ms: uint64` | ✅ |

### §17 XOR-метрика
| Требование | Статус |
|-----------|--------|
| `xor_distance = A XOR B` (big-endian) | ✅ |
| Беззнаковое 256-битное сравнение | ✅ `[u8;32]` лексикографически = BE unsigned |
| Нет строкового / знакового сравнения | ✅ |

### §18 Таблица маршрутизации
| Требование | Статус |
|-----------|--------|
| 256 фиксированных bucket | ✅ |
| Ёмкость = K_BUCKET_SIZE | ✅ |
| Голова = LRU, хвост = новейший | ✅ `push_back` / `pop_front` |
| Известный контакт → переносится в хвост | ✅ T2.7 |
| Живой LRU → `PING` перед вытеснением | ✅ `ping_lru()` возвращает LRU для проверки |
| Мёртвый LRU → вытесняется | ✅ T2.9 `evict_lru_and_insert` |
| Локальный NodeID не попадает в таблицу | ✅ `retain(x.node_id != own_id)` |

### §19 PING
| Требование | Статус |
|-----------|--------|
| PONG содержит тот же `request_id` | ✅ `Frame::new_response()` |
| PONG содержит собственный контакт | ✅ `PongPayload::responder` |
| PONG содержит `ping_timestamp_ms` из запроса | ✅ |
| После PONG контакт обновляется в таблице | ✅ `update_routing()` |
| Тайм-аут PING_TIMEOUT_MS | ✅ |

### §20 FIND_NODE
| Требование | Статус |
|-----------|--------|
| Ответ содержит ≤ K_BUCKET_SIZE контактов | ✅ `find_closest(target, k)` |
| Контакты отсортированы по XOR-расстоянию | ✅ `sort_by_key(xor_distance)` |
| Локальный узел не включается в ответ | ✅ `retain(x != own_id)` |

### §21 Итеративный lookup
| Требование | Статус |
|-----------|--------|
| α=3 параллельных запроса | ✅ |
| Остановка при отсутствии прогресса | ✅ |
| Запись rpc_count, iterations | ✅ `LookupResult` |

### §22/§25 Bootstrap
| Требование | Статус |
|-----------|--------|
| Star-схема | ✅ `StarBootstrap` |
| Ring-схема | ✅ `RingBootstrap` |
| PING → self-lookup при join | ✅ `DhtNode::join()` |
| 5+ конфигов узлов | ✅ `config/nodes/node-0{1..5}.yaml` |
| Скрипты автозапуска | ✅ `scripts/run_star.sh`, `run_ring.sh` |

### §27 Критические нарушения — все устранены
| Нарушение | Статус |
|-----------|--------|
| «одно read = один кадр» | ✅ нет (FrameReader) |
| Нет проверки MAX до выделения памяти | ✅ нет (проверяется строка 194) |
| `request_id` не используется | ✅ нет (PendingRpc) |
| NodeID не связан с pubkey | ✅ нет (SHA-256) |
| Закрытые ключи в репо | ✅ нет (state/ в .gitignore) |
| k-bucket не ограничены | ✅ нет (capacity) |
| Живой LRU вытесняется без PING | ✅ нет (ping_lru) |
| FIND_NODE возвращает полный список | ✅ нет (K ближайших) |
| Нет автотестов | ✅ нет (32 теста) |

### §28 Расширяемость
| Требование | Статус |
|-----------|--------|
| STORE/FIND_VALUE без смены заголовка | ✅ коды 0x05-0x08 зарезервированы |
| TLS без переписывания RoutingTable | ✅ `FramedStream<S>` generic |
| Туннели | ✅ уже реализованы (Фаза 6) |
| Экспорт метрик | ✅ уже реализован (Фаза 8) |

---

## ⚠️ Частично / Требует доработки

### §9 — Примеры payload в PROTOCOL.md
**ТЗ:** «привести минимум по одному примеру payload для каждого типа»  
**Есть:** схемы структур. **Нет:** конкретных примеров в JSON/hex.  
**Трудозатраты:** ~30 мин

### §15.3 / §18.3 — Отклонение дубликата NodeID с другим pubkey
**ТЗ:** «Контакт с тем же NodeID, но иным открытым ключом отклоняется»  
**Есть:** поле `identity_public_key` в Contact. **Нет:** явной проверки при `update_routing()`.  
**Трудозатраты:** ~1 час

### §26 — Краткий отчёт 3–5 страниц
**ТЗ:** обязателен к приёмке этапа 2.  
**Статус:** отсутствует (`docs/defense/` есть, но не отчёт).  
**Трудозатраты:** ~2–3 часа

---

## ❌ Отсутствует

### Тесты T2.10, T2.12, T2.13, T2.14
| Тест | Что проверяет |
|------|---------------|
| **T2.10** | PING/PONG: ответ имеет тот же `request_id`, контакт корректен |
| **T2.12** | Bootstrap: после PING + self-lookup есть ≥1 подтверждённый контакт |
| **T2.13** | Многопереходный lookup через промежуточный узел |
| **T2.14** | После отключения bootstrap-узла lookup между другими работает |

> T2.13 и T2.14 — **integration-тесты**, требуют запуска реальных узлов (tokio::test).

### §25.3 — Экспорт таблиц маршрутизации в файл
**ТЗ:** «выгрузка таблиц маршрутизации всех узлов», «результаты 10+ lookup в JSON/CSV»  
**Есть:** `export_routing_table()` в `DhtNode`. **Нет:** вызова из main/node при старте/остановке.

---

## Итог

| Категория | Кол-во | % |
|-----------|--------|---|
| ✅ Выполнено | ~90 требований | ~87% |
| ⚠️ Частично | 3 пункта | ~10% |
| ❌ Отсутствует | T2.10,12,13,14 + экспорт файлов | ~5% |

**Критических нарушений (§27): 0**  
**Для приёмки необходимо:** добавить T2.10/T2.12 + примеры payload в PROTOCOL.md
