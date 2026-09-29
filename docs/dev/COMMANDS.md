# Справочник команд проекта

> **Навигация:** [docs/README.md](../README.md)  
> Все команды выполняются из корня репозитория.

---

## Сборка

```bash
# Собрать в debug-режиме
cargo build

# Собрать в release-режиме (быстрее, меньше бинарник)
cargo build --release

# Проверить без сборки (быстро)
cargo check
```

---

## Запуск узла

```bash
cargo run --bin p2p-node -- [ОПЦИИ]
```

### Опции CLI

| Флаг | Короткий | По умолчанию | Описание |
|------|----------|-------------|---------|
| `--config <PATH>` | `-c` | `config/node_config.yaml` | Путь к YAML-конфигу |
| `--port <PORT>` | `-p` | из конфига | Переопределить LISTEN_PORT |
| `--bootstrap <ADDR>` | `-b` | из конфига | Добавить bootstrap-пир (можно несколько) |

### Примеры

```bash
# Seed-узел (первый, без bootstrap)
cargo run --bin p2p-node -- --config config/nodes/node-01.yaml

# Узел с переопределённым портом
cargo run --bin p2p-node -- --config config/nodes/node-02.yaml --port 7002

# Узел с явным bootstrap-пиром
cargo run --bin p2p-node -- --port 7003 --bootstrap 127.0.0.1:7001

# Несколько bootstrap-пиров
cargo run --bin p2p-node -- --port 7004 \
  --bootstrap 127.0.0.1:7001 \
  --bootstrap 127.0.0.1:7002

# Release-сборка (для нагрузочного теста)
cargo run --release --bin p2p-node -- --config config/nodes/node-01.yaml
```

---

## Уровень логирования

```bash
# Переменная окружения RUST_LOG (приоритет над config.node.log_level)
RUST_LOG=p2p_overlay=debug cargo run --bin p2p-node -- --config config/nodes/node-01.yaml

# Только warnings
RUST_LOG=p2p_overlay=warn cargo run --bin p2p-node -- ...

# Всё подробно (включая зависимости)
RUST_LOG=debug cargo run --bin p2p-node -- ...

# Конкретные модули
RUST_LOG=p2p_overlay::dht=debug,p2p_overlay::rpc=info cargo run --bin p2p-node -- ...
```

Уровни: `error` < `warn` < `info` < `debug` < `trace`

---

## Тесты

```bash
# Все тесты (с выводом в logs/)
bash scripts/run_tests.sh

# Все тесты напрямую через cargo
cargo test

# С выводом println!/info! в консоль
cargo test -- --nocapture

# Конкретный тест
cargo test t1_5_payload_too_large

# Конкретный файл тестов
cargo test --test test_framing
cargo test --test test_identity
cargo test --test test_routing
cargo test --test test_tunnel

# С фильтром по имени
cargo test t2_   # все тесты T2.*
cargo test t1_   # все тесты T1.*

# Показать время выполнения каждого теста
cargo test -- -Z unstable-options --report-time
```

---

## Стенды (несколько узлов)

```bash
# Star bootstrap (рекомендуется для демонстрации)
bash scripts/run_star.sh        # 5 узлов (по умолчанию)
bash scripts/run_star.sh 13     # N узлов

# Ring bootstrap
bash scripts/run_ring.sh
bash scripts/run_ring.sh 13

# Логи → logs/star/<timestamp>/ или logs/ring/<timestamp>/
```

---

## Эксперименты

```bash
# Все 9 сценариев подряд
bash experiments/run_all.sh
bash experiments/run_all.sh 5   # N узлов

# Отдельные сценарии
bash experiments/scenarios/01_cold_start.sh
bash experiments/scenarios/02_find_node.sh
bash experiments/scenarios/03_store_find_value.sh
bash experiments/scenarios/04_keeper_failure.sh
bash experiments/scenarios/05_lookup_failure.sh
bash experiments/scenarios/06_tunnel_message.sh
bash experiments/scenarios/07_relay_failure.sh
bash experiments/scenarios/08_negative_crypto.sh
bash experiments/scenarios/09_bootstrap_compare.sh

# Логи → logs/<scenario>/<timestamp>/
# Результаты → experiments/results/ (CSV/JSON)
```

---

## Диагностика

```bash
# Проверить warnings компилятора
cargo build 2>&1 | grep "^warning"

# Проверить clippy (расширенные lint'ы)
cargo clippy

# Форматирование кода
cargo fmt
cargo fmt -- --check   # проверка без изменений

# Граф зависимостей
cargo tree

# Размер бинарника
ls -lh target/debug/p2p-node
ls -lh target/release/p2p-node   # после cargo build --release
```

---

## Документация

```bash
# Сгенерировать rustdoc (HTML)
cargo doc --open

# Только для нашего крейта (без зависимостей)
cargo doc --no-deps --open
```

---

## Git

```bash
# История коммитов
git log --oneline -10

# Статус
git status --short

# Просмотр изменений
git diff src/node/mod.rs

# Коммит
git add -A && git commit -m "feat: ..."
```

---

## Структура папки logs/

```
logs/
├── star/
│   └── 20261001_120000/
│       ├── node-01.log
│       ├── node-02.log
│       └── ...
├── ring/
│   └── 20261001_130000/
│       └── ...
├── tests/
│   └── 20261001_140000.log    ← cargo test output
├── 01_cold_start/
│   └── 20261001_150000/
│       └── results.log
└── run_all_20261001_160000.log ← полный suite лог
```

> `logs/` добавлена в `.gitignore` — логи не попадают в репозиторий.
