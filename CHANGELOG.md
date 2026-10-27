# Changelog

Все изменения проекта. Дата = дата фактического применения кода.

---

## [2026-10-27] feat: DHT §10.1/§10.3 — TTL-очистка, переопубликование, sequence
- `evict_expired` теперь вызывается периодически (каждые 10 с) — раньше нигде не вызывался.
- Переопубликование: узел хранит свои записи (`DhtNode::published`) и раз в `republish_interval_seconds` публикует их заново с новым sequence.
- `DhtRecord.sequence` входит в подпись (домен `P2P-DHT-RECORD-v2`); передаётся в STORE и FIND_VALUE. Откат версии и подмена sequence отвергаются.
- Тесты: `tests/test_dht_records.rs` (5).
- Не сделано: полноценный `NodeRecord` с `issued_at/expires_at`; критерий «STORE успешен при ≥2 из 3».

## [2026-10-27] feat: воспроизводимая приёмка этапов 1–2
- `p2p-orchestrator acceptance` + `scripts/run_acceptance.sh`: N узлов, сходимость, выгрузка таблиц, контрольные lookup (цель вне таблицы), отказ bootstrap, FIND_VALUE — реальные измерения в `experiments/results/accept_*/`
- lookup: трассировка опрошенных узлов по итерациям, `target_known_before`, `found_target`
- независимые тест-векторы `docs/test_vectors/`, краткий отчёт `docs/defense/Report_Etapy_1-2.md`
- сценарии 02–09 писали случайные данные (`$RANDOM`) — нарушение ТЗ §22: оставлены как отключённые каркасы (`exit 2`), не удалены; логи узлов приёмки — в `logs/` (вне git)

## [2026-10-27] feat: TLS 1.3 подключён к транспорту + негативные тесты
- Раньше `security/tls.rs` нигде не вызывался — весь P2P-трафик шёл по голому TCP. Теперь `dial()` и accept-цикл узла работают поверх TLS 1.3 (mutual, Ed25519).
- Исправлено: NodeID пира из сертификата считался как SHA-256 от DER (не совпадал с identity NodeID). Теперь Ed25519-ключ берётся из SPKI (x509-parser), NodeID = SHA-256(0x01‖pk).
- PING: NodeID в PONG обязан совпадать с ключом TLS-сертификата.
- Исправлено: при двух крипто-провайдерах rustls (ring/aws-lc) узел падал при старте — `ring` выбирается явно.
- `UnexpectedEof` без close_notify трактуется как обычный EOF.
- Тесты: `tests/test_tls.rs` (7) — подмена NodeID, не-TLS клиент, мусор, клиент без сертификата, TLS 1.2, битый cert.
- Ограничения: TLS-контекст общий на процесс (`rpc::install_tls`); NodeID пира сверяется только в PING, остальные RPC шифруются и аутентифицируются ключом, но без сверки с ожидаемым NodeID.

## [2026-10-26] fix: A4–A8 (чат, relay-пересылка, подписи DHT)
- A4/A5: `Messenger` подключён к `Node`; TUNNEL_DATA пересылается relay по цепочке (`next_hop`, `TunnelRoutes`), конечный узел отдаёт данные в Messenger
- A6: проверка подписи сообщения по `sender_pubkey` (не зависит от routing table), анти-replay
- A7: подписанные DHT-записи, first-writer-wins
- A8: STORE/FIND_VALUE больше не заглушки; `DhtNode::put/get`
- GUI: отправка сообщений и входящие; STORE/FIND через DHT

## [2026-10-26] feat: GUI + аудит раунд 2
- Веб-панель узла (`--gui-port`) и веб-оркестратор кластера (`p2p-orchestrator`), см. docs/dev/GUI.md
- A2: проверка NodeID/pubkey в PONG и контактах FIND_NODE_RESPONSE
- A3: анонс `127.0.0.1` вместо `0.0.0.0`
- Аудит: docs/dev/CODE_AUDIT_2.md

## [2026-10-26] fix: B3 + B6 (туннели)
> Аналоги E22 и E1-6 в tunnel/. Статус всех замечаний: docs/dev/FIXED_REVIEW_COMMENTS.md
- B3: проверка NodeID/pubkey инициатора (relay) и relay (инициатор), проверка типа ответа и request_id; сессия использует согласованный tunnel_id (`TunnelSession::with_id`)
- B6: приёмник исходящей очереди хранится в сессии (`attach_outbox`/`take_outbox`), а не отбрасывается

## [unreleased] — 2026-10-12

### fix(E1-6, E21-E22, E24-E26) — критические исправления RPC и идентичности

> Правки получены и применены **12.10.2026**.
> Часть из них — по замечаниям ревью (веб-версия, оффлайн-правки).

#### E1-6 / E24-E26 — Чтение пакетов в исходящих соединениях (`src/rpc/mod.rs`)
- **Проблема:** RpcClient отправлял запрос, но никто не читал ответ
  из TCP-потока — `pending.deliver()` никогда не вызывался, все RPC
  падали по тайм-ауту.
- **Решение:** функция `spawn_conn_reader(stream, pending)` запускает
  фоновую задачу чтения на каждое исходящее соединение.
  `ping()` и `find_node()` теперь принимают `SocketAddr`, сами вызывают
  `dial()` → `send()` → `spawn_conn_reader()`.

#### E22 — Проверка NodeID/pubkey при каждом входящем сообщении (`src/rpc/mod.rs`)
- `handle_ping`, `handle_find_node`, `handle_store`, `handle_find_value`
  вызывают `verify_node_id_bytes()` перед добавлением в routing table.
- Несовпадение `NodeID ≠ SHA-256(pubkey)` → `warn + bail!` + ERROR-кадр.

#### E21 — Тесты сохранности NodeID (`tests/test_identity.rs`)
- `e21_node_id_stable_across_restarts` — NodeID не меняется при restart
- `e21_different_dirs_different_ids` — разные dirs → разные ID
- `e22_verify_node_id_valid_pubkey` — корректный pubkey → true
- `e22_verify_node_id_wrong_pubkey_rejected` — чужой pubkey → false

---

## [2026-10-12] feat: A1+A5+C3+C4+T4 — архитектурные улучшения

> Коммит `d3e05c1`. Написан **01.10.2026**, залит в репозиторий **12.10.2026**
> (был потерян локально, применён через веб/оффлайн правку).

#### A5 — First-K-wins lookup (`src/dht/lookup.rs`)
- `join_all()` заменён на `FuturesUnordered` — ответы обрабатываются по мере прихода.

#### A1 — Bucket refresh (`src/dht/routing_table.rs`, `src/node/mod.rs`)
- `random_targets_per_bucket()` + `tokio::interval` → периодический lookup
  для поддержания актуальности routing table.

#### C3 — `TunnelManager::monitor_loop()` в `Node::start()` (`src/node/mod.rs`)
- Туннели теперь реально мониторятся и перестраиваются при падении.

#### C4 — `MetricsCollector` пишет CSV (`src/dht/node.rs`, `src/dht/lookup.rs`)
- Каждый `find_node()` → `lookup_{node_id}.csv`.

#### T4 — Graceful shutdown Ctrl+C (`src/node/mod.rs`)
- `watch::channel` + `tokio::signal::ctrl_c()` + flush метрик при выходе.

---

## [2026-09-29] fix+docs — критические правки по ТЗ + команды + логи

- PROTOCOL.md: `0xFF` → `0x7F` для ERROR
- `handle_conn()`: закрытие соединения при ошибке кадра (§10 ТЗ)
- 0 warnings компилятора (был 21)
- `--export-routing <PATH>` в CLI (§26 ТЗ)
- `docs/dev/COMMANDS.md` — справочник всех команд
- `docs/dev/IMPROVEMENTS.md` — аудит улучшений
- `scripts/run_tests.sh` — тесты с записью в `logs/tests/`

---

## [2026-09-28] fix(tz-compliance) — проверка по ТЗ этапов 1-2

- §18.3: дубликат NodeID с другим pubkey отклоняется в `update_routing`
- §9: примеры payload в PROTOCOL.md
- §24: тесты T2.10–T2.14
- 36/36 тестов ✅

---

## [2026-09-28] chore — логи + Defense_Etapy_1-2 обновлён

- `logs/` в `.gitignore`, все скрипты пишут в `logs/<scenario>/`
- `Defense_Etapy_1-2.md` — чек-лист защиты, 36 тестов, §26 материалы

---

## [2026-09-28] docs — Theory_Etapy_1-2.md подробная теория

- Цели проекта, термины (DHT, Kademlia, XOR-метрика, k-bucket, overlay)
- Ed25519, SHA-256, framing, tunnel — каждый термин объяснён

