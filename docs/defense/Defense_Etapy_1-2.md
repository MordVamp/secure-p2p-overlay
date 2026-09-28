# Защита этапов 1 и 2

> **Навигация:** [docs/README.md](../README.md) · [Краткая теория](Theory.md) · [**Подробная теория 1–2**](Theory_Etapy_1-2.md)  
> **ТЗ:** [requirements/TZ_Etapy_1-2.md](../requirements/TZ_Etapy_1-2.md) · [Устные требования](../requirements/Ustnie_Trebovaniya.md)  
> **Матрица соответствия:** [GAP_TZ_Full_Check.md](../requirements/GAP_TZ_Full_Check.md)  
> **Перекрёстные ссылки:** [dev/CROSSLINKS.md](../dev/CROSSLINKS.md)

---

## Быстрый статус перед защитой

```bash
cargo test          # 36/36 ✅ (T1×10, T2×9, T6×6, T7×6)
cargo build         # Finished ✅
bash scripts/run_star.sh   # Логи → logs/star/<timestamp>/
```

**§27 критических нарушений:** 0 из 11 ✅

---

## Что демонстрировать — Этап 1 (Транспорт и кадрирование)

### 1. Формат кадра → [`src/transport/framing.rs`](../../src/transport/framing.rs)

```
HEADER_SIZE = 24 байта  MAX_FRAME_PAYLOAD = 65 536  PROTOCOL_VERSION = 1
│version│type│  flags  │         request_id (16B)         │payload_len│ payload…
│  1B   │ 1B │   2B    │               16B                │    4B     │  0..64K
```

Показать:
- `FrameReader` — `BytesMut`-буфер, конечный автомат (не «одно read = один кадр»)
- Строка `194`: `payload_length > MAX_FRAME_PAYLOAD` → отклоняем **до** выделения буфера (защита от OOM)
- `MsgType::Error = 0x7F` (не 0xFF — зарезервированное значение §8 ТЗ)
- `Frame::new_response()` — копирует `request_id` из запроса (§7 ТЗ)

### 2. Запустить тесты Этапа 1

```bash
cargo test test_framing -- --nocapture
```

| Тест | Что проверяет |
|------|--------------|
| T1.1 | Частичный заголовок буферизован — не паника |
| T1.3 | Два склеенных кадра разделены корректно |
| T1.4 | Фрагментированный кадр (3 TCP-чтения) собран |
| T1.5 | `payload_length > MAX` → `ReadItem::Error`, без OOM |
| T1.6 | Неверная версия → отклонено |
| T1.9 | PING round-trip: encode → decode → payload корректен |

### 3. Конфигурация → [`config/node_config.yaml`](../../config/node_config.yaml)

Показать все 12 параметров §5 ТЗ. Акцент: `k_bucket_size` и `alpha` — только в YAML, без правки кода.

### 4. Типовые вопросы — Этап 1

| Вопрос преподавателя | Ответ |
|---------------------|-------|
| Почему 24B заголовок? | version(1)+type(1)+flags(2)+request_id(16)+length(4) = 24 |
| Почему UUID для `request_id`? | Корреляция запрос/ответ + дедупликация replay (§7) |
| Почему MessagePack, а не JSON? | 32B NodeID → 33B (MP) vs 44B (JSON base64), быстрее парсинг |
| Что при `partial read`? | `FrameReader` накапливает в `BytesMut`, ждёт следующих байт |
| Что при `payload_length > 65536`? | Проверяем ДО `buf.reserve()` → `ReadItem::Error` без выделения |

---

## Что демонстрировать — Этап 2 (Идентичность и DHT)

### 1. NodeID → [`src/identity/node_identity.rs`](../../src/identity/node_identity.rs)

```rust
NodeID = SHA-256(0x01 ‖ pubkey_bytes)  // 0x01 = контекстный домен Ed25519
```

Показать:
- `load_or_create()` — ключ из `{state_dir}/identity.key.pem`, права `0600`
- `state/` в `.gitignore` → ключи никогда не в репозитории (§27 ТЗ)

### 2. RoutingTable + KBucket → [`src/dht/routing_table.rs`](../../src/dht/routing_table.rs)

```
256 бакетов (фиксированные, по старшему биту XOR).
Голова VecDeque = LRU (самый старый), хвост = самый свежий.
```

Ключевые методы:
- `update()`: insert в хвост / перемещение в хвост / PING LRU при полном бакете
- `evict_lru_and_insert()`: только если LRU не ответил на PING
- **§18.3**: `update_routing()` в `rpc/mod.rs` — отклоняет контакт с тем же NodeID но другим pubkey

### 3. Итеративный lookup → [`src/dht/lookup.rs`](../../src/dht/lookup.rs)

```
α=3 параллельных FIND_NODE → объединяем → берём α новых ближайших
Стоп: ближайший не улучшился за раунд → возвращаем K ближайших
Результат: LookupResult { rpc_count, iterations, duration_ms, closest }
```

### 4. Bootstrap → [`src/dht/node.rs`](../../src/dht/node.rs) · [`src/bootstrap/mod.rs`](../../src/bootstrap/mod.rs)

```bash
bash scripts/run_star.sh 5    # логи → logs/star/<timestamp>/
```

Показать:
- `join()`: PING bootstrap-адресов → self-lookup → таблица заполнена
- Star vs Ring — `scripts/run_star.sh` / `scripts/run_ring.sh`
- 5 конфигов: `config/nodes/node-01.yaml` … `node-05.yaml`

### 5. Запустить тесты Этапа 2

```bash
cargo test test_identity test_routing -- --nocapture
```

| Тест | §ТЗ | Что проверяет |
|------|-----|--------------|
| T2.1 | §14 | NodeID стабилен при рестарте |
| T2.3 | §14 | NodeID детерминирован из pubkey |
| T2.4 | §14 | Несоответствие NodeID/pubkey → Err |
| T2.5 | §17 | XOR-метрика: эталонные векторы |
| T2.6 | §18.2 | Вставка в неполный бакет — в хвост |
| T2.7 | §18.2 | Известный контакт → в хвост (LRU обновлён) |
| T2.8 | §18.2 | Полный бакет + живой LRU → новый отклонён |
| T2.9 | §18.2 | Полный бакет + мёртвый LRU → вытеснен, новый добавлен |
| T2.10 | §19 | PING/PONG: тот же `request_id`, `ping_timestamp` echoed |
| T2.11 | §20 | `find_closest()` → отсортировано по XOR |
| T2.12 | §25 | После bootstrap routing table непуста |
| T2.13 | §25.3 | Lookup через промежуточный узел (target неизвестен инициатору) |
| T2.14 | §25.2 | После отключения bootstrap-узла lookup работает |

### 6. Экспорт routing table (§26 ТЗ)

```bash
# В коде: DhtNode::export_routing_table() → serde_json::Value
# Вывод в JSON — демонстрация невырожденности
```

### 7. Типовые вопросы — Этап 2

| Вопрос преподавателя | Ответ |
|---------------------|-------|
| Как работает поиск в DHT? | Итеративный Kademlia: α=3 параллельных FIND_NODE, стоп при отсутствии прогресса, O(log N) RPC |
| Почему LRU а не FIFO? | Долгоживущие узлы статистически надёжнее (Kademlia §2.4) — они прошли больше PING |
| Что если bootstrap-узел упадёт? | После self-lookup узел независим — T2.14 это доказывает |
| Как защититесь от Sybil? | NodeID = SHA-256(pubkey) → нельзя выбрать произвольный ID без знания ключа |
| Почему lookup не вырождается? | Каждый узел знает ≤K=3 в каждом бакете, итого ≤12 из N=13–15. Инициатор не знает всех |
| §18.3 — что если два узла с одним NodeID? | `update_routing()` проверяет `identity_public_key` — при расхождении: warn + reject |

---

## Структура сдаваемых материалов (§26 ТЗ)

| Требование §26 | Файл/директория | Статус |
|---------------|-----------------|--------|
| Исходный код | `src/` | ✅ |
| `README.md` с последовательностью запуска | `README.md` | ✅ |
| `ARCHITECTURE.md` | `docs/implementation/ARCHITECTURE.md` | ✅ |
| `PROTOCOL.md` + примеры payload | `docs/implementation/PROTOCOL.md` | ✅ |
| Конфиги ≥5 узлов | `config/nodes/node-0{1..5}.yaml` | ✅ |
| Сценарий автозапуска и остановки | `scripts/run_star.sh`, `run_ring.sh` | ✅ |
| Unit + integration тесты | `tests/` (36 тестов) | ✅ |
| Журналы контрольного запуска | `logs/` (в .gitignore, создаётся при запуске) | ✅ |
| Экспорт routing table JSON | `DhtNode::export_routing_table()` | ✅ |
| Результаты lookup CSV | `experiments/results/` | ✅ |
| **Краткий отчёт 3–5 стр.** | ❌ | нет |

---

## Чек-лист перед защитой

```
[ ] cargo test        →  36/36 ok
[ ] cargo build       →  Finished ok
[ ] bash scripts/run_star.sh 5   →  5 узлов запустились, логи в logs/star/
[ ] Открыть: docs/requirements/TZ_Etapy_1-2.md  (для ссылок на §§)
[ ] Открыть: docs/requirements/Ustnie_Trebovaniya.md  (приоритеты)
[ ] Открыть: docs/requirements/GAP_TZ_Full_Check.md  (матрица соответствия)
[ ] Подготовить терминал: 3 вкладки — код / тесты / запуск стенда
[ ] При необходимости показать экспорт: cargo run -- --export-routing
```
