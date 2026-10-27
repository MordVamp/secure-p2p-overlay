# Changelog

Все изменения проекта. Дата = дата фактического применения кода.

---

## [2026-10-27] docs: README, USERGUIDE, THREAT_MODEL, analyze.py
- Новые `USERGUIDE.md`, `docs/THREAT_MODEL.md` (с остаточными рисками); README переписан.
- `scripts/analyze.py`: графики и stats.md из CSV приёмки; вызывается из `run_acceptance.sh`.

## [2026-10-27] feat: APP_ACK (подтверждение доставки) + диагностика маршрута; fix: получатель из lookup
- `APP_ACK` (§13.1): получатель после расшифровки и проверки подписи шлёт подписанное, E2E-зашифрованное подтверждение (`MessageKind::Ack`, payload = message_id). `Messenger::send_text_acked` ждёт его; ACK засчитывается только от адресата, подделка/повтор отвергаются, на ACK ACK не шлётся, в inbox ACK не попадает.
- GUI: `/api/send` возвращает `delivered` и `rtt_ms`; новый `/api/route?target=…` — диагностика маршрута (туннель, цепочка relay, живость hop, давность последнего ACK).
- **Исправлена ошибка:** `send` брал адрес получателя только из таблицы маршрутизации. При малом k корзина может быть полна и не удержать узел, который lookup нашёл → «target unknown — сначала lookup» (воспроизводилось в части запусков). Теперь контакт и ключ берутся из результата lookup (`build_with`/`get_or_build_with`), пара NodeID/pubkey проверяется.
- Лимит текста уменьшен на 1 KiB под накладные расходы Envelope.
- Тесты: `tests/test_app_ack.rs` (9). Всего 100 тестов, 0 упавших.
- Живая проверка: 4 раунда × 3 пары на 6 узлах (12/12 доставлено с подтверждением, RTT 91–176 мс).

## [2026-10-27] feat: max_hops и защита от петель (§12)
- `TUNNEL_BUILD` на relay: `1 <= total_hops <= tunnel.max_hops`, `hop_index < total_hops`, `next_hop != инициатор` (петля), `tunnel_id` уникален (повторный BUILD не перезаписывает маршрут). Раньше `max_hops` был только в конфиге.
- `TUNNEL_DATA`: поле `hops_left` (по умолчанию 0 → не пересылается). Отправитель ставит `max_hops+1`, relay уменьшает на 1 и отбрасывает кадр при 0. Зацикленные таблицы пересылки больше не дают бесконечной циркуляции.
- Builder: кандидаты уникальны (каждый узел в пути ≤ 1 раза); `min_relays > max_hops` отвергается.
- Тесты: `tests/test_loops.rs` (10), включая реальный цикл A→B→A на сокетах и проверку декремента `hops_left`.
- Живая проверка: 6 узлов, сообщение A→B доходит.
- Совместимость: формат `TUNNEL_DATA` изменился (узлы старой версии не перешлют кадр).

## [2026-10-27] feat: сквозное шифрование сообщений (§11.2) + журнал архитектурных решений
- `security/e2e.rs`: X25519 (эфемерный) + HKDF-SHA256 + AES-256-GCM, подпись внутри сообщения; relay видят только `Envelope`. Открытые сообщения не принимаются.
- `Messenger::send_text` шифрует для получателя; ключ берётся из таблицы маршрутизации или из `NodeRecord` в DHT.
- Тесты: `tests/test_e2e.rs` (9). Живая проверка: 6 узлов, A→B через туннель, текст получен и расшифрован.
- Новый файл `docs/dev/ARCHITECTURE_DECISIONS.md` — все архитектурные решения сессии.

## [2026-10-27] feat: NodeRecord в DHT (§10.3) + кворум STORE (§10.1)
- `NodeRecord` (node_id, pubkey, addresses, sequence_number, issued_at, expires_at, подпись) теперь реально используется: хранится в DHT под `SHA-256("node:"||NodeID)`, значение `NREC1`+msgpack.
- `DhtNode::publish_node_record` (после bootstrap и при каждом republish, с новыми issued_at/expires_at) и `resolve_node` (подпись, TTL, привязка к ключу, совпадение NodeID).
- `handle_store`: NodeRecord принимается только валидным, от владельца, под своим ключом, без отката `sequence_number`.
- Кворум: `dht.store_quorum` (по умолчанию 2) — `put` возвращает ошибку, если подтверждено меньше 2 реплик из R=3.
- Тесты: `tests/test_node_record.rs` (11): подмена адреса/ключа, чужой ключ DHT, истёкшая запись, issued_at из будущего, откат sequence, кворум.
- Проверено живой приёмкой N=13: все критерии ✅; 12/13 узлов опубликовали запись (первый узел при пустой сети получает 1/3 и обновляет запись при следующем цикле).
- Ограничение: чужой публикатор может занять ключ `node:<victim>` данными без префикса NREC1 (first-writer-wins); смягчается TTL.

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

