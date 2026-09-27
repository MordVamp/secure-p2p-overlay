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

## 🔧 Разработка

| Документ | Описание |
|----------|----------|
| [dev/PLAN.md](dev/PLAN.md) | Пошаговый план реализации по фазам (рабочий) |

---

## 📁 Остальные материалы проекта

```
experiments/
  scenarios/         ← 9 сценариев эксперимента (01–09)
  run_all.sh         ← запуск всех сценариев одной командой
  results/           ← CSV/JSON результаты (в .gitignore)

tests/               ← 32 автоматических теста (T1–T7)
config/nodes/        ← конфиги для 5 узлов
scripts/             ← run_star.sh, run_ring.sh
src/                 ← исходный код (37 файлов Rust)
```
