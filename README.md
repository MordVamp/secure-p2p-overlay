# Secure P2P Overlay — курсовая работа

Узел защищённой оверлейной P2P-сети: **Kademlia DHT** + **TLS 1.3 (mutual, Ed25519)** + **туннели через relay** + **сквозное шифрование сообщений**.

**Язык:** Rust 2021 · **Транспорт:** TCP + TLS 1.3 · **DHT:** Kademlia (k=3, α=3, R=3) · **Уровень:** simplified

> 📚 [Документация](docs/README.md) · 📖 [USERGUIDE](USERGUIDE.md) · 🛡️ [Модель угроз](docs/THREAT_MODEL.md) · 🧭 [Архитектурные решения](docs/dev/ARCHITECTURE_DECISIONS.md) · 📝 [CHANGELOG](CHANGELOG.md)

---

## Одна команда: сеть → приёмка → результаты

```bash
bash scripts/run_acceptance.sh 13 30     # N=13 узлов, 30 контрольных lookup
```

Скрипт собирает проект, поднимает N узлов (первый — seed, остальные знают только его адрес), ждёт сходимости, выгружает таблицы маршрутизации, делает контрольные lookup, **останавливает bootstrap-узел и повторяет lookup**, затем строит графики.

| Что | Где |
|---|---|
| Сводка и критерии ✅/❌ | `experiments/results/accept_*/summary.md` |
| Сырые данные lookup | `lookups_before.csv/json`, `lookups_after_bootstrap_loss.csv/json` |
| Таблицы маршрутизации | `routing_tables/`, `routing_summary.csv` |
| Статистика и графики | `stats.md`, `plots/*.png` (`python3 scripts/analyze.py [каталог]`) |
| Логи узлов (**не в git**) | `logs/accept_*/` |

Все числа — реальные измерения прогона; в `summary.md` записаны коммит и параметры. Отчёт: [docs/defense/Report_Etapy_1-2.md](docs/defense/Report_Etapy_1-2.md).

## Быстрый старт

```bash
cargo build --bins                 # требуется Rust 1.75+, Linux/macOS
cargo test                         # 100+ тестов, включая негативные (TLS, E2E, записи DHT, петли)

# один узел с веб-панелью
cargo run --bin p2p-node -- --port 7001 --gui-port 8001      # http://127.0.0.1:8001/

# второй узел, bootstrap к первому
cargo run --bin p2p-node -- --port 7002 -b 127.0.0.1:7001 --gui-port 8002 \
    --state-dir state/n2 --metrics-dir metrics/n2

# кластер с веб-интерфейсом оркестратора
cargo run --bin p2p-orchestrator -- --gui-port 8080          # http://127.0.0.1:8080/
```

Подробно — в [USERGUIDE.md](USERGUIDE.md).

## Что реализовано

| Область | Реализация |
|---|---|
| Кадры и протокол | заголовок 24 Б, MessagePack, лимит payload 64 КиБ, проверка версии → [PROTOCOL](docs/implementation/PROTOCOL.md) |
| Идентичность | Ed25519; `NodeID = SHA-256(0x01‖pubkey)`, пара NodeID/pubkey проверяется везде |
| DHT | k-buckets, итеративный lookup (α=3), bootstrap, refresh; STORE/FIND_VALUE с R=3 и кворумом ≥2; TTL-очистка и republish |
| Записи | подпись публикатора, `sequence` (откат отвергается), `NodeRecord` (issued_at/expires_at) |
| Канал | **TLS 1.3**, mutual auth, NodeID из SPKI сертификата, без 0-RTT и TLS 1.2 |
| Туннели | построение через ≥2 relay, `max_hops`, `hops_left` (защита от петель), уникальность `tunnel_id` |
| Сообщения | **E2E** (X25519 + HKDF + AES-256-GCM), подпись Ed25519, anti-replay, **APP_ACK**, диагностика маршрута |
| Метрики | CSV/JSON по lookup, туннелям; экспорт таблиц маршрутизации |
| GUI | локальная веб-панель узла и оркестратор (без внешних зависимостей, только `127.0.0.1`) |

Что **не** реализовано или с ограничениями — см. [модель угроз](docs/THREAT_MODEL.md#остаточные-риски) и [COMPLIANCE](docs/requirements/COMPLIANCE_2026-10-26.md).

## Структура проекта

```
src/
  config.rs, types.rs      — конфигурация (YAML), NodeId, Contact
  protocol/, transport/    — payload-структуры (MessagePack), кадрирование, FramedStream
  identity/                — Ed25519, NodeID, NodeRecord
  security/                — tls.rs (TLS 1.3), e2e.rs (сквозное шифрование)
  dht/                     — routing table, lookup, DhtNode, хранилище записей
  rpc/                     — PING/FIND_NODE/STORE/FIND_VALUE, TUNNEL_*, dial()
  tunnel/                  — builder, session, forwarder, routes, manager
  app/                     — Messenger (E2E-сообщения, APP_ACK)
  node/, gui/, metrics/    — сервер узла, веб-панель, сбор метрик
  acceptance.rs, orchestrator.rs — приёмочный прогон и оркестратор кластера
tests/                     — unit/интеграционные и негативные тесты
config/                    — node_config.yaml и примеры
scripts/                   — run_acceptance.sh, analyze.py, run_star.sh, run_ring.sh, run_tests.sh
experiments/               — сценарии (этапы 3+) и результаты приёмки
docs/                      — ТЗ, архитектура, протокол, защита, dev-заметки
```

## Безопасность в двух словах

- Весь трафик между узлами — TLS 1.3 с взаимной аутентификацией; ключ в сертификате = identity-ключ узла.
- Relay видят только зашифрованный контейнер; получатель проверяет подпись отправителя.
- Приватные ключи лежат в `state/` — каталог в `.gitignore`, в репозиторий не попадает.
- Логи узлов — в `logs/` (тоже вне git).

Подробно и честно про пробелы: [docs/THREAT_MODEL.md](docs/THREAT_MODEL.md).
