# Руководство пользователя

Все команды — из корня репозитория. Требуется Rust 1.75+ (Linux/macOS); для графиков — Python 3 + matplotlib.

## 1. Сборка и проверка

```bash
cargo build --bins        # p2p-node и p2p-orchestrator
cargo test                # все тесты
bash scripts/run_tests.sh # то же, с записью в logs/tests/
```

## 2. Запуск узла

```bash
cargo run --bin p2p-node -- [ОПЦИИ]
```

| Опция | Описание |
|---|---|
| `-c, --config <PATH>` | YAML-конфиг (по умолчанию `config/node_config.yaml`) |
| `-p, --port <PORT>` | переопределить `LISTEN_PORT` |
| `-b, --bootstrap <ADDR>` | bootstrap-пир `IP:PORT` (можно несколько) |
| `--gui-port <PORT>` | включить веб-панель на `127.0.0.1:<PORT>` (без флага GUI выключен) |
| `--state-dir <DIR>` | каталог состояния (там лежит приватный ключ узла) |
| `--metrics-dir <DIR>` | каталог метрик |
| `--export-routing <FILE>` | выгрузить таблицу маршрутизации в JSON и выйти |

Первый узел запускается без `-b` (seed), остальные — с `-b <адрес seed>`. **Каждому узлу нужны свой порт и свой `--state-dir`**, иначе узлы получат один и тот же ключ.

Минимальная сеть из 6 узлов:

```bash
P=./target/debug/p2p-node
$P -p 7301 --gui-port 8301 --state-dir state/n1 --metrics-dir metrics/n1 &
for i in 2 3 4 5 6; do
  $P -p 730$i -b 127.0.0.1:7301 --gui-port 830$i \
     --state-dir state/n$i --metrics-dir metrics/n$i &
  sleep 0.5
done
# сходимость занимает ~20–30 с; остановить: pkill -x p2p-node
```

Уровень логов: `RUST_LOG=p2p_overlay=debug cargo run --bin p2p-node -- ...`.

## 3. Веб-панель узла (`--gui-port`)

Откройте `http://127.0.0.1:<gui-port>/`: статус и NodeID, таблица маршрутизации по корзинам, туннели, хранилище DHT. Действия: PING, FIND_NODE, построение туннеля, STORE/FIND, отправка сообщения.

Те же функции доступны по HTTP (удобно для скриптов). Изменяющие запросы — `POST`:

| Запрос | Описание |
|---|---|
| `GET /api/status` | NodeID, адрес, таблица маршрутизации, туннели, хранилище |
| `POST /api/ping?addr=IP:PORT` | PING по адресу |
| `POST /api/lookup?target=<NodeID>` | итеративный FIND_NODE |
| `POST /api/tunnel?target=<NodeID>` | построить туннель |
| `POST /api/store?key=...&value=...` | подписанная запись DHT с репликацией (R=3, кворум ≥2) |
| `GET /api/find?key=...` | поиск записи (проверяется подпись публикатора) |
| `POST /api/send?target=<NodeID>&text=...` | отправить сообщение и **дождаться подтверждения** → `{"delivered":true,"rtt_ms":…}` |
| `GET /api/inbox` | входящие сообщения |
| `GET /api/route?target=<NodeID>` | диагностика маршрута: туннель, цепочка relay, живость hop |

GUI слушает только `127.0.0.1` и **не имеет аутентификации** — не пробрасывайте порт наружу.

Пример (из сети выше):

```bash
B=$(curl -s localhost:8306/api/status | python3 -c "import sys,json;print(json.load(sys.stdin)['node_id'])")
curl -s -X POST "localhost:8302/api/send?target=$B&text=hello"
# {"delivered":true,"ok":true,"rtt_ms":143}
curl -s localhost:8306/api/inbox
```

Для отправки сообщения нужны минимум 4 известных узла: отправитель, ≥2 relay (`tunnel.min_relays`) и получатель.

## 4. Оркестратор (кластер из браузера)

```bash
cargo build --bins && cargo run --bin p2p-orchestrator -- --gui-port 8080
# http://127.0.0.1:8080/
```

Запуск N узлов (первый — seed, остальные знают только его адрес), остановка одного или всех, ссылки на панели узлов (порт узла + 1000).

## 5. Приёмка и результаты

```bash
bash scripts/run_acceptance.sh [N=13] [LOOKUPS=30]
python3 scripts/analyze.py [experiments/results/accept_...]   # повторно построить графики
```

Состав результатов описан в [README](README.md#одна-команда-сеть--приёмка--результаты). Логи узлов — в `logs/<имя_прогона>/` (вне git).

## 6. Конфигурация

`config/node_config.yaml`; ключевые параметры:

| Параметр | По умолчанию | Смысл |
|---|---|---|
| `dht.k_bucket_size` | 3 | размер корзины (simplified=3, advanced=4) |
| `dht.alpha` | 3 | параллелизм lookup |
| `dht.replication_factor` / `store_quorum` | 3 / 2 | STORE успешен при ≥2 из 3 реплик |
| `dht.ttl_seconds` / `republish_interval_seconds` | 180 / 80 | срок жизни записи и период переопубликования |
| `transport.connect_timeout_ms` / `read_timeout_ms` | 3000 / 5000 | тайм-ауты |
| `tunnel.min_relays` / `max_hops` | 2 / 5 | длина цепочки relay |
| `tunnel.ttl_seconds` / `ack_timeout_ms` | 600 / 5000 | срок жизни туннеля, тайм-аут подтверждения |

Параметры меняются без правки кода; CLI-флаги `-p/-b/--state-dir` переопределяют файл.

## 7. Данные, ключи, логи

| Что | Где | В git? |
|---|---|---|
| Приватный ключ узла | `<state-dir>/` | **нет** (`state/` в `.gitignore`) |
| Логи узлов и тестов | `logs/` | нет |
| Метрики узла | `<metrics-dir>/` | нет |
| Результаты приёмки | `experiments/results/accept_*/` | да (без `state/`) |

Потеряли каталог состояния — узел получит **новый** NodeID. Не копируйте `state/` между машинами.

## 8. Частые проблемы

| Симптом | Причина / решение |
|---|---|
| `bind ...: Address already in use` | порт занят; выберите другой `-p` или `pkill -x p2p-node` |
| у двух узлов одинаковый NodeID | общий `--state-dir`; задайте отдельный каждому |
| `Failed to build tunnel ... target unknown` | сеть ещё не сошлась (подождите 20–30 с) или известно <4 узлов |
| `нет подтверждения доставки` | получатель недоступен/выключен или маршрут разорван; проверьте `/api/route` |
| `STORE: подтверждено 1 из 3 реплик` | в сети слишком мало узлов для кворума (нужно ≥2 реплик) |
| графики не строятся | `pip install matplotlib` |
| `rustc: ... libLLVM ...` при сборке | сломан системный Rust (обновите пакеты `rust`/`llvm`) |
