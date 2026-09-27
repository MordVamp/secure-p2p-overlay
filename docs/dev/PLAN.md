# План выполнения курсовой работы
## «Защищённая оверлейная сеть передачи данных на основе пиринговых протоколов»

> **Связанные документы:**  
> 📋 [ТЗ этапов 1-2](../requirements/TZ_Etapy_1-2.md) — официальные требования  
> 🗣 [Устные требования](../requirements/Ustnie_Trebovaniya.md) — приоритеты и пояснения с лекций  
> 🔍 [GAP-анализ](../requirements/GAP_Analysis.md) — что было недостающим  
> 🏗 [Архитектура](../implementation/ARCHITECTURE.md) · [Протокол](../implementation/PROTOCOL.md)

---

**Язык:** Rust 1.98+ (tokio, rustls, ed25519-dalek)  
**Уровень:** Упрощённый (N=12–15, K=3, ретрансляторов ≥ 2, 2 bootstrap-схемы)  
**Архитектура:** расширяема до продвинутого уровня без переписывания — через трейты и конфиг

---

## Статус фаз

| Фаза | Содержание | Статус | Недели ТЗ |
|------|-----------|--------|-----------|
| **0** | Проектное решение, конфиг, общие типы | ✅ Done | 1–2 |
| **1** | TCP-транспорт и кадрирование | ✅ Done | 1–2 |
| **2** | Идентичность узла (Ed25519, NodeRecord) | ✅ Done | 3–4 |
| **3** | TLS 1.3 + replay-защита | ✅ Done | 7–8 |
| **4** | Kademlia DHT (k-bucket, lookup, STORE/FIND_VALUE) | 🚧 routing_table готова | 3–6 |
| **5** | Bootstrap-схемы (star, ring) | ⬜ TODO | 11–12 |
| **6** | Туннельная передача + восстановление | ⬜ TODO | 9–10 |
| **7** | Прикладной мессенджер (E2E) | ⬜ TODO | 9–10 |
| **8** | Метрики и эксперименты (9 сценариев) | ⬜ TODO | 11–14 |
| **9** | Docker + README + run_acceptance.sh | ⬜ TODO | 11–12 |
| **10** | Документация, записка | ⬜ TODO | 14–15 |

---

## Фаза 0 — Проектное решение ✅

### Файлы
- [`src/config.rs`](../src/config.rs) — все параметры в одном месте, YAML-конфиг
- [`src/types.rs`](../src/types.rs) — `NodeId`, `Contact`, `now_unix()`
- [`config/node_config.yaml`](../config/node_config.yaml) — дефолтный конфиг

### Ключевые решения

| Параметр | Упрощённый | Продвинутый | Где меняется |
|----------|-----------|-------------|-------------|
| N | 12–15 | 20–21 | `docker-compose.yml` |
| K_BUCKET_SIZE | 3 | 4 | `config/node_config.yaml` |
| ALPHA | 3 | 3 | конфиг |
| R | 3 | 3 | конфиг |
| Ретрансляторов | ≥ 2 | ≥ 3 | конфиг `tunnel.min_relays` |
| Пул туннелей | 1 | ≥ 3 | конфиг `tunnel.pool_size` |
| Защита канала | TLS 1.3 | кастомный AKE | `config.level` → трейт |
| Bootstrap-схем | 2 | 4 | добавить impl BootstrapScheme |

### Расширяемость без рефакторинга
- **`ChannelSecurity` trait** (`src/security/mod.rs`) → добавить `ake.rs` рядом с `tls.rs`
- **`BootstrapScheme` trait** (Фаза 5) → добавить `tree.rs`, `multi_seed.rs`
- **`NodeLevel` enum** в конфиге → меняет параметры в рантайме
- **Все лимиты** (K, α, R, TTL, pool_size) → только YAML-конфиг

---

## Фаза 1 — Транспорт и кадрирование ✅

### Файлы
- [`src/transport/framing.rs`](../src/transport/framing.rs)
- [`src/transport/connection.rs`](../src/transport/connection.rs)

### Формат кадра (24-байтный заголовок, big-endian)

```
┌──────────┬──────────┬──────────────┬──────────────────────────┬────────────────┐
│ version  │ msg_type │    flags     │       request_id          │ payload_length │
│  1 байт  │  1 байт  │   2 байта    │        16 байт            │    4 байта     │
└──────────┴──────────┴──────────────┴──────────────────────────┴────────────────┘
│                           payload (0..65536 байт)                               │
```

### Типы сообщений

| Hex | Тип | Направление |
|-----|-----|-------------|
| 0x01/02 | PING / PONG | DHT keepalive |
| 0x03/04 | FIND_NODE_REQUEST/RESPONSE | DHT lookup |
| 0x05/06 | STORE_REQUEST/RESPONSE | DHT store |
| 0x07/08 | FIND_VALUE_REQUEST/RESPONSE | DHT get |
| 0x10–0x15 | TUNNEL_BUILD/OK/FAIL/DATA/ACK/CLOSE | Туннель |
| 0x20/21 | APP_MESSAGE / APP_ACK | Приложение |
| 0xFF | ERROR | Ошибки |

### Гарантии корректности
- `payload_length` проверяется **до** выделения буфера (защита от DoS)
- `FrameReader` — конечный автомат, корректно собирает разбитые/склеённые чтения
- Неизвестный тип/версия → `ReadItem::Error`, сброс одного байта + ресинхронизация

---

## Фаза 2 — Идентичность узла ✅

### Файлы
- [`src/identity/node_identity.rs`](../src/identity/node_identity.rs)
- [`src/identity/record.rs`](../src/identity/record.rs)

### NodeID

```
NodeID = SHA-256( 0x01 ‖ Ed25519_public_key_bytes )
                  ^^^^
               префикс типа ключа (canonical encode)
```

- Стабилен между перезапусками (ключ сохраняется в `state_dir/identity.key`)
- При получении чужого pubkey: пересчёт NodeID и сравнение — обязательно

### NodeRecord (структура DHT-записи)

```rust
NodeRecord {
    node_id,          // NodeId (32 байта)
    identity_pubkey,  // Ed25519 pubkey (32 байта)
    addresses[],      // SocketAddr — где принимает соединения
    sequence_number,  // u64 — монотонно растёт (защита от rollback)
    issued_at,        // Unix timestamp
    expires_at,       // Unix timestamp (TTL 120–300 с)
    signature,        // Ed25519 подпись canonical_bytes()
    // TODO (продвинутый): alias: Option<String>
}
```

### Валидация записи (все проверки обязательны)
1. Восстановить `VerifyingKey` из `identity_pubkey`
2. Пересчитать NodeID → сравнить с `node_id`
3. Проверить Ed25519-подпись `canonical_bytes()`
4. Проверить TTL (`now < expires_at`)
5. Проверить `issued_at` (не из далёкого будущего, ≤ now+60s)
6. При merge: `incoming.seq > stored.seq` → обновить; иначе — `SequenceRollback`

---

## Фаза 3 — Защита канала ✅

### Файлы
- [`src/security/tls.rs`](../src/security/tls.rs) — TLS 1.3 mutual auth
- [`src/security/session_tracker.rs`](../src/security/session_tracker.rs) — replay-защита
- [`src/security/mod.rs`](../src/security/mod.rs) — трейт `ChannelSecurity`

### Трейт-точка расширения

```rust
// Упрощённый уровень:   TlsSecurity  (уже реализован)
// Продвинутый уровень:  AkeSecurity  (добавить src/security/ake.rs)
//   ├── Ed25519 долговременная подпись
//   ├── X25519 эфемерная пара на сессию
//   ├── HKDF-SHA-256 с контекстом протокола
//   ├── AES-256-GCM или ChaCha20-Poly1305
//   └── Транскрипт: version ‖ roles ‖ NodeID_A ‖ NodeID_B ‖ eph_A ‖ eph_B ‖ nonce_A ‖ nonce_B ‖ session_id
pub trait ChannelSecurity { ... }
```

### TLS 1.3 (упрощённый уровень)
- Только TLS 1.3 (TLS 1.2 явно отклонён)
- 0-RTT отключён (нет session tickets — защита от межсоединительного replay)
- Взаимная аутентификация (mutual TLS)
- Self-signed X.509 на Ed25519 ключе узла (сгенерирован через `rcgen`)
- После handshake: извлечь pubkey peer → пересчитать NodeID → сравнить
- End-to-end для прикладных данных: отдельная TLS-сессия поверх туннеля (Фаза 7)

### Replay-защита (SessionTracker)
- Sliding window по времени (TTL из конфига, по умолчанию 3600 с)
- Каждый `request_id` (16 байт UUID) регистрируется при первом получении
- Повторный `request_id` в окне → отклонение без прикладного действия

---

## Фаза 4 — Kademlia DHT 🚧

### Реализовано
- [`src/dht/routing_table.rs`](../src/dht/routing_table.rs) — `KBucket` + `RoutingTable`
  - XOR-метрика через `NodeId::xor_distance()` и `NodeId::bucket_index()`
  - LRU-порядок в bucket (голова = свежий, хвост = LRU для PING)
  - `find_closest(target, k)` — отсортировано по XOR

### TODO (следующий коммит)
- `storage.rs` — DHT-хранилище, TTL, валидация перед записью
- `rpc.rs` — сообщения PING/FIND_NODE/STORE/FIND_VALUE (MessagePack)
- `lookup.rs` — итеративный lookup, α=3 параллельных запроса, журнал
- `node.rs` — фасад: join/put/get/refresh/republish

### Критерий невырожденности (ТЗ §6)
- ≥ 80% узлов после сходимости имеют < N-1 контактов
- ≥ 30 lookup, в которых цель отсутствует у инициатора до старта
- Каждый такой lookup использует ≥ 1 промежуточный узел
- Bootstrap-узел можно отключить — DHT продолжает работу

---

## Фазы 5–10 — TODO

### Фаза 5 — Bootstrap-схемы
```rust
pub trait BootstrapScheme: Send + Sync {
    fn initial_peers(&self, node_index: usize, all_addrs: &[SocketAddr]) -> Vec<SocketAddr>;
}
// Упрощённый: StarBootstrap, RingBootstrap
// Продвинутый: + TreeBootstrap, MultiSeedBootstrap
```

### Фаза 6 — Туннель
```
Состояния: BUILDING → ACTIVE → DEGRADED → CLOSING → DEAD
Упрощённый: min_relays = 2, pool_size = 1
Продвинутый: min_relays = 3, pool_size ≥ 3, профили кандидатов
```

### Фаза 7 — Прикладные сервисы
```
Упрощённый:   текстовые сообщения, E2E TLS поверх туннеля
Продвинутый:  + file_transfer.rs (блоки ≤ 48 KiB, SHA-256 верификация)
```

### Фаза 8 — Эксперименты (9 обязательных сценариев)
```
01_cold_start         ≥ 5 запусков/конфиг
02_find_node          ≥ 30 запросов
03_store_find_value   R=3 реплики
04_keeper_failure     ≥ 5 инъекций
05_lookup_failure     промежуточный узел во время lookup
06_tunnel_message     ≥ 30 наблюдений
07_relay_failure      ≥ 5 инъекций, автовосстановление
08_negative_crypto    8 обязательных отрицательных тестов
09_bootstrap_compare  star vs ring при неизменных факторах
```

### Фаза 9 — Docker
```bash
docker compose up --scale node=15   # N=15 узлов
./run_acceptance.sh                  # одна команда → полный прогон
```

### Фаза 10 — Документация
- `docs/architecture.md` — схема подсистем (4 графа: underlay/bootstrap/DHT/tunnel)
- `docs/threat_model.md` — модель угроз, явные границы
- Пояснительная записка (15 обязательных разделов по ТЗ §20)

---

## Структура репозитория (итоговая)

```
kursovay/
├── src/
│   ├── lib.rs                  ← объявление модулей
│   ├── main.rs                 ← бинарник узла
│   ├── orchestrator.rs         ← бинарник оркестратора
│   ├── config.rs               ← ✅ конфиг NodeConfig
│   ├── types.rs                ← ✅ NodeId, Contact
│   ├── transport/              ← ✅ Фаза 1
│   │   ├── framing.rs
│   │   └── connection.rs
│   ├── identity/               ← ✅ Фаза 2
│   │   ├── node_identity.rs
│   │   └── record.rs
│   ├── security/               ← ✅ Фаза 3
│   │   ├── mod.rs              (трейт ChannelSecurity)
│   │   ├── tls.rs              (упрощённый уровень)
│   │   ├── session_tracker.rs
│   │   └── [ake.rs]           ← TODO продвинутый уровень
│   ├── dht/                    ← 🚧 Фаза 4
│   │   ├── routing_table.rs    (готова)
│   │   ├── [storage.rs]
│   │   ├── [rpc.rs]
│   │   ├── [lookup.rs]
│   │   └── [node.rs]
│   ├── [bootstrap/]            ← TODO Фаза 5
│   ├── [tunnel/]               ← TODO Фаза 6
│   ├── [app/]                  ← TODO Фаза 7
│   └── [metrics/]              ← TODO Фаза 8
├── tests/                      ← unit + integration тесты
│   ├── [test_framing.rs]
│   ├── [test_identity.rs]
│   ├── [test_tls.rs]
│   └── [test_dht.rs]
├── experiments/
│   ├── scenarios/              ← 9 обязательных сценариев
│   └── results/                ← CSV/JSON/PNG (в .gitignore)
├── config/
│   ├── node_config.yaml
│   ├── bootstrap_star.yaml     ← TODO Фаза 5
│   └── bootstrap_ring.yaml     ← TODO Фаза 5
├── docs/
│   ├── PLAN.md                 ← этот файл
│   ├── design_decision.md      ← выбор уровня, языка, форматов
│   ├── protocol_spec.md        ← формат кадра, типы сообщений
│   ├── [architecture.md]       ← TODO Фаза 10
│   └── [threat_model.md]       ← TODO Фаза 10
├── Cargo.toml
├── Dockerfile                  ← TODO Фаза 9
├── docker-compose.yml          ← TODO Фаза 9
├── run_acceptance.sh           ← TODO Фаза 9
└── README.md                   ← TODO Фаза 9
```

---

## Критические запреты (ТЗ §22)

> ❌ Готовая DHT-библиотека — только своя реализация k-bucket/lookup  
> ❌ Собственные крипто-примитивы — только `rustls`, `ed25519-dalek`, `ring`  
> ❌ Закрытые ключи в репозитории (`state/` в `.gitignore`)  
> ❌ Глобальный реестр NodeID→addr в рабочем узле  
> ❌ Прямой fallback засчитывается как успешный туннель  
> ❌ Графики без исходных CSV/JSON
