# Документация проекта

Курсовая работа: «Защищённая оверлейная сеть передачи данных на основе пиринговых протоколов»

---

## 📋 Требования

| Документ | Описание |
|----------|----------|
| [TZ_Etapy_1-2.md](requirements/TZ_Etapy_1-2.md) | Официальное ТЗ этапов 1–2 (от преподавателя) |
| [Ustnie_Trebovaniya.md](requirements/Ustnie_Trebovaniya.md) | Неформальные требования и приоритеты с лекций |
| [GAP_Analysis.md](requirements/GAP_Analysis.md) | Что не хватало по этапам 1–2 — gap-анализ |

> **Про ТЗ:** официальное ТЗ — ориентир, не абсолют.
> Подробнее: [Ustnie_Trebovaniya.md §5](requirements/Ustnie_Trebovaniya.md)

---

## 🏗 Реализация

| Документ | Описание |
|----------|----------|
| [ARCHITECTURE.md](implementation/ARCHITECTURE.md) | Архитектура системы, модульная схема, инженерные решения |
| [PROTOCOL.md](implementation/PROTOCOL.md) | Формат кадра, типы сообщений, DHT-ключи |
| [DESIGN_DECISIONS.md](implementation/DESIGN_DECISIONS.md) | Выбор языка, уровня, сериализации, криптографии |

---

## 🛡 Защита

| Документ | Описание |
|----------|----------|
| [defense/Theory.md](defense/Theory.md) | Теория (краткая): DHT, Kademlia, TLS, туннели, метрики |
| [defense/Theory_Etapy_1-2.md](defense/Theory_Etapy_1-2.md) | **Подробная теория Этапов 1–2** — каждый термин, цели, архитектурные решения |
| [defense/Defense_Etapy_1-2.md](defense/Defense_Etapy_1-2.md) | Что показывать и рассказывать на защите этапов 1–2 |

---

## 🔧 Разработка

| Документ | Описание |
|----------|----------|
| [dev/PLAN.md](dev/PLAN.md) | Пошаговый план реализации по фазам (рабочий) |
| [dev/CROSSLINKS.md](dev/CROSSLINKS.md) | Все перекрёстные ссылки и причины их существования |
| [dev/IMPROVEMENTS.md](dev/IMPROVEMENTS.md) | Аудит улучшений: критичные, технические, концептуальные |
| [dev/COMMANDS.md](dev/COMMANDS.md) | Справочник всех команд (CLI, тесты, скрипты) |
| [dev/ARCHITECTURE_DECISIONS.md](dev/ARCHITECTURE_DECISIONS.md) | Журнал архитектурных решений (ADR-01…25) |
| [dev/GUI.md](dev/GUI.md) | Веб-панель узла, оркестратор, настольное приложение |
| [dev/plannow.md](dev/plannow.md) | Текущий план: что сделано, что осталось |
| [requirements/COMPLIANCE_2026-10-26.md](requirements/COMPLIANCE_2026-10-26.md) | Соответствие ТЗ по коду (обновляется) |
| [THREAT_MODEL.md](THREAT_MODEL.md) | Модель угроз, остаточные риски, проверка pcap |

---

## 📁 Остальные материалы проекта

```
experiments/
  scenarios/         ← 9 сценариев эксперимента (01–09), обёртки над scripts/scenarios.py
  run_all.sh         ← запуск всех сценариев одной командой
  results/           ← приёмка (accept_*) и сценарии (NN_*/<время>/: results.csv, summary.md, meta.json)

desktop/             ← настольное приложение p2p-desktop (egui), работает через HTTP API узла
docs/img/            ← снимки экрана приложения (для USERGUIDE)
tests/               ← 113 автоматических тестов, включая негативные
config/nodes/        ← конфиги для 5 узлов
scripts/             ← run_acceptance.sh, scenarios.py, analyze.py, run_star.sh, run_ring.sh
src/                 ← исходный код узла (Rust)
```
