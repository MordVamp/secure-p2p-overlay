# Secure P2P Overlay — Курсовая работа

Узел защищённой оверлейной P2P-сети на основе Kademlia DHT.  
**Язык:** Rust 2021 | **Транспорт:** TCP | **DHT:** Kademlia | **Уровень:** Simplified (N=12–15, K=3)

---

## Быстрый старт

### 1. Сборка

```bash
cargo build
# или в оптимизированном режиме:
cargo build --release
```

### 2. Запуск одного узла (seed)

```bash
cargo run --bin p2p-node -- --config config/nodes/node-01.yaml
```

### 3. Запуск стенда из 5 узлов (Star bootstrap)

```bash
bash scripts/run_star.sh
# Логи: logs/node-0{1..5}.log
```

### 4. Ring bootstrap

```bash
bash scripts/run_ring.sh
```

### 5. Запуск тестов

```bash
cargo test
# T1.1–T1.10 (транспорт), T2.1–T2.9, T2.11 (DHT + identity)
```

---

## Параметры запуска

```
p2p-node [OPTIONS]

Options:
  -c, --config <PATH>       YAML-файл конфигурации [default: config/node_config.yaml]
  -p, --port <PORT>         Переопределить LISTEN_PORT
  -b, --bootstrap <ADDR>    Добавить bootstrap-пир (IP:PORT), можно несколько раз
  -h, --help
```

### Переменные среды

```bash
RUST_LOG=p2p_overlay=debug  # уровень логирования
```

---

## Структура проекта

```
src/
  config.rs          — конфигурация (YAML)
  types.rs           — NodeId, Contact
  protocol/          — payload-структуры, MessagePack кодеки
  transport/         — кадрирование TCP, FrameReader, FramedStream
  identity/          — Ed25519 ключи, NodeId = SHA-256(pubkey)
  dht/               — k-bucket, RoutingTable, итеративный lookup, DhtNode
  rpc/               — PING/PONG, FIND_NODE обработчики, PendingRpc
  node/              — TCP-сервер, диспетчер сообщений
  bootstrap/         — Star и Ring схемы
  security/          — TLS 1.3 (Фаза 3)
tests/
  test_framing.rs    — T1.1–T1.10
  test_identity.rs   — T2.1–T2.5
  test_routing.rs    — T2.6–T2.9, T2.11
config/
  node_config.yaml   — конфиг по умолчанию
  nodes/             — конфиги для 5 узлов
scripts/
  run_star.sh        — Star bootstrap стенд
  run_ring.sh        — Ring bootstrap стенд
docs/
  PLAN.md            — план выполнения
  GAP_Etapy_1_2.md   — что было недостающим
  protocol_spec.md   — спецификация протокола
```

---

## Формат кадра (§7 ТЗ)

| Смещение | Поле            | Размер | Описание                    |
|:--------:|-----------------|:------:|-----------------------------|
| 0        | `version`       | 1 B    | Должно быть `1`             |
| 1        | `type`          | 1 B    | Тип сообщения               |
| 2        | `flags`         | 2 B    | Флаги (0 на этапах 1–2)     |
| 4        | `request_id`    | 16 B   | UUID v4 (CSPRNG)            |
| 20       | `payload_length`| 4 B    | 0..65536 (big-endian)       |
| 24       | `payload`       | var    | MessagePack                 |

## Типы сообщений (§8 ТЗ)

| Код    | Тип                  |
|--------|----------------------|
| `0x01` | `PING`               |
| `0x02` | `PONG`               |
| `0x03` | `FIND_NODE_REQUEST`  |
| `0x04` | `FIND_NODE_RESPONSE` |
| `0x7F` | `ERROR`              |

---

## Требования

- Rust 1.75+
- Linux/macOS
