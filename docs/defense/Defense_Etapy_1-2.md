# Защита этапов 1 и 2

> **Навигация:** [docs/README.md](../README.md) · [Теория](Theory.md)  
> **ТЗ:** [requirements/TZ_Etapy_1-2.md](../requirements/TZ_Etapy_1-2.md) · [Устные требования](../requirements/Ustnie_Trebovaniya.md)

---

## Что демонстрировать на защите

### Этап 1 — Транспорт и кадрирование

#### Показать в коде
1. **Формат кадра** → [`src/transport/framing.rs`](../../src/transport/framing.rs)
   - Заголовок 24B: version, msg_type, flags, request_id (UUID), payload_length
   - `FrameReader` — конечный автомат, буфер `BytesMut`
   - Константы: `HEADER_SIZE=24`, `MAX_FRAME_PAYLOAD=65536`, `PROTOCOL_VERSION=1`

2. **Типы сообщений** → [`src/transport/framing.rs`](../../src/transport/framing.rs) (enum `MsgType`)
   - ERROR = `0x7F` (не 0xFF — важно, это было исправлено в gap-анализе)
   - Флаги: `IS_RESPONSE=0x1`, `IS_ERROR=0x2`

3. **Защита от OOM** — `payload_length` проверяется ДО выделения буфера

4. **Конфиг** → [`config/node_config.yaml`](../../config/node_config.yaml)
   - `max_frame_payload`, `connect_timeout_ms`, `read_timeout_ms`

#### Запустить тесты
```bash
cargo test test_framing -- --nocapture
```
Показать прохождение **T1.1–T1.10**:
- T1.1: частичный заголовок буферизован
- T1.3: два склеенных кадра разделены корректно
- T1.4: фрагментированный кадр (3 TCP read) собран
- T1.5: `payload_length > MAX` → ReadItem::Error (не паника, не OOM)
- T1.6: неверная версия → Error
- T1.9: PING round-trip через MessagePack

#### Объяснить ключевые решения
| Вопрос | Ответ |
|--------|-------|
| Почему 24B заголовок? | version(1) + type(1) + flags(2) + request_id(16) + length(4) = 24 |
| Почему UUID для request_id? | Корреляция запрос/ответ + дедупликация (replay-защита) |
| Почему MessagePack? | Компактнее JSON для бинарных NodeID, быстрее парсинг |
| Что делать при partial read? | FrameReader держит буфер, ждёт следующих данных |

---

### Этап 2 — Идентичность и DHT

#### Показать в коде
1. **NodeIdentity** → [`src/identity/node_identity.rs`](../../src/identity/node_identity.rs)
   - `NodeID = SHA-256(0x01 ‖ pubkey_bytes)` — контекстный домен
   - `load_or_create()` — загрузка/создание ключа, права 0600
   - `sign()` / `verify_signature()` — Ed25519

2. **RoutingTable + KBucket** → [`src/dht/routing_table.rs`](../../src/dht/routing_table.rs)
   - 256 бакетов по старшему биту XOR-расстояния
   - LRU-порядок: голова = самый старый
   - `find_closest(target, k)` → отсортировано по XOR

3. **Итеративный lookup** → [`src/dht/lookup.rs`](../../src/dht/lookup.rs)
   - α=3 параллельных FIND_NODE
   - Остановка при отсутствии прогресса

4. **Bootstrap** → [`src/dht/node.rs`](../../src/dht/node.rs)
   - `join()`: PING bootstrap-адресов → self-lookup

5. **Bootstrap-схемы** → [`src/bootstrap/mod.rs`](../../src/bootstrap/mod.rs)
   - `StarBootstrap`, `RingBootstrap`

#### Запустить тесты
```bash
cargo test test_identity test_routing -- --nocapture
```
Показать **T2.1–T2.9, T2.11**:
- T2.1: NodeID стабилен при рестарте
- T2.3: NodeID детерминирован из pubkey
- T2.4: несовпадение NodeID/pubkey → Err
- T2.7: известный контакт перемещается в хвост (LRU обновляется)
- T2.8: полный бакет + живой LRU → новый отклонён
- T2.9: полный бакет + мёртвый LRU → вытеснен, новый добавлен
- T2.11: `find_closest()` возвращает отсортировано по XOR

#### Объяснить ключевые решения
| Вопрос | Ответ |
|--------|-------|
| Почему Ed25519? | Быстрая верификация, малый размер ключа (32B), аудированная библиотека |
| Почему NodeID = SHA-256(pubkey)? | Нельзя выбрать произвольный ID без знания ключа (Sybil-защита) |
| Почему XOR-метрика? | Симметрична, детерминирована, O(log N) шагов к цели |
| Почему LRU а не FIFO в k-bucket? | Долгоживущие узлы статистически надёжнее (Kademlia §2.4) |
| Зачем самолукап при join? | Заполнить ближние бакеты и объявить о себе соседям |

---

## Что рассказывать (план ответов на вопросы)

### «Как работает поиск в вашей DHT?»
> Используем итеративный Kademlia lookup. Инициатор берёт α=3 ближайших из локальной таблицы,
> параллельно отправляет им FIND_NODE. Каждый возвращает K ближайших, которых он знает.
> Инициатор объединяет ответы, берёт новые α ближайших, повторяет.
> Останавливается когда ближайший не улучшился. Сложность: O(log N) RPC.

### «Как вы защищаете от replay-атак?»
> Каждый запрос содержит request_id (UUID v4, 16 байт из CSPRNG).
> SessionTracker хранит sliding window полученных ID на TTL секунд.
> Повторный request_id → отклоняется без прикладного действия.
> Дополнительно: TLS 1.3 с 0-RTT off исключает replay на уровне handshake.

### «Что будет если bootstrap-узел упадёт?»
> Bootstrap нужен только при первом join. После того как узел заполнил таблицу
> маршрутизации через self-lookup — он не зависит от bootstrap.
> Это можно проверить сценарием 04 (keeper failure).

### «Почему lookup не вырождается?»
> Условие: инициатор не знает target до старта lookup.
> Это гарантируется структурой DHT: у каждого узла K=3 контакта в каждом бакете,
> итого максимум ~12 из N=13–15. Инициатор не знает всех → lookup нетривиален.
> Проверяется сценарием 02: 30 lookup с промежуточными узлами.

---

## Чек-лист перед защитой

- [ ] `cargo test` — все 32 теста проходят
- [ ] `cargo build` — без ошибок
- [ ] `bash scripts/run_star.sh` — 5 узлов запускаются
- [ ] Подготовить терминал с 3 вкладками: код / тесты / запуск
- [ ] Открыть [`docs/requirements/TZ_Etapy_1-2.md`](../requirements/TZ_Etapy_1-2.md) — для ссылки на §§
- [ ] Открыть [`docs/requirements/Ustnie_Trebovaniya.md`](../requirements/Ustnie_Trebovaniya.md) — приоритеты
