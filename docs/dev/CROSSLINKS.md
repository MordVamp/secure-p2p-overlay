# Перекрёстные ссылки проекта

> Этот файл — единственный источник истины по навигации между документами.  
> При добавлении нового файла — обновляй сначала этот документ, затем расставляй ссылки.  
> Упоминается в: [docs/README.md](../README.md)

---

## Структура документов и связи

```
docs/
├── README.md                        ← Центральный индекс (hub)
│     ├── → requirements/TZ_Etapy_1-2.md
│     ├── → requirements/Ustnie_Trebovaniya.md
│     ├── → requirements/GAP_Analysis.md
│     ├── → implementation/ARCHITECTURE.md
│     ├── → implementation/PROTOCOL.md
│     ├── → implementation/DESIGN_DECISIONS.md
│     └── → dev/PLAN.md
│
├── requirements/
│   ├── TZ_Etapy_1-2.md              ← Официальное ТЗ
│   │     ├── → Ustnie_Trebovaniya.md
│   │     └── → GAP_Analysis.md
│   ├── Ustnie_Trebovaniya.md        ← Неформальные требования
│   │     ├── → TZ_Etapy_1-2.md
│   │     └── → GAP_Analysis.md
│   └── GAP_Analysis.md             ← Gap-анализ этапов 1-2
│         ├── → TZ_Etapy_1-2.md
│         └── → Ustnie_Trebovaniya.md
│
├── implementation/
│   ├── ARCHITECTURE.md              ← Архитектура системы
│   │     ├── → requirements/TZ_Etapy_1-2.md  (откуда требования)
│   │     ├── → requirements/Ustnie_Trebovaniya.md
│   │     ├── → PROTOCOL.md         (детали протокола)
│   │     └── → DESIGN_DECISIONS.md (обоснование выборов)
│   ├── PROTOCOL.md                  ← Спецификация кадра и типов
│   └── DESIGN_DECISIONS.md         ← Выбор языка/крипто
│
├── dev/
│   ├── PLAN.md                      ← Рабочий план фаз
│   │     ├── → requirements/TZ_Etapy_1-2.md
│   │     ├── → requirements/Ustnie_Trebovaniya.md
│   │     ├── → requirements/GAP_Analysis.md
│   │     ├── → implementation/ARCHITECTURE.md
│   │     └── → implementation/PROTOCOL.md
│   └── CROSSLINKS.md               ← Этот файл
│
└── defense/
    ├── Theory.md                    ← Теория для защиты
    │     └── → Defense_Etapy_1-2.md
    └── Defense_Etapy_1-2.md        ← Что показывать/рассказывать
          ├── → Theory.md
          ├── → requirements/TZ_Etapy_1-2.md
          ├── → requirements/Ustnie_Trebovaniya.md
          └── → src/ (прямые ссылки на исходники)

README.md (корневой)
    ├── → docs/README.md
    └── → docs/requirements/TZ_Etapy_1-2.md
        → docs/requirements/Ustnie_Trebovaniya.md
```

---

## Таблица ссылок с обоснованием

| Откуда | Куда | Причина |
|--------|------|---------|
| `README.md` (корень) | `docs/README.md` | Навигация в документацию из главной страницы |
| `README.md` (корень) | `requirements/TZ_Etapy_1-2.md` | Быстрый доступ к ТЗ из GitHub главной |
| `README.md` (корень) | `requirements/Ustnie_Trebovaniya.md` | Быстрый доступ к неформальным требованиям |
| `docs/README.md` | все 7 файлов docs/ | Центральный индекс — точка входа во всю доку |
| `PLAN.md` | `TZ_Etapy_1-2.md` | Фазы плана строятся по требованиям ТЗ |
| `PLAN.md` | `Ustnie_Trebovaniya.md` | Приоритеты фаз исходят из устных требований |
| `ARCHITECTURE.md` | `TZ_Etapy_1-2.md` | Архитектурные решения обоснованы §§ ТЗ |
| `ARCHITECTURE.md` | `PROTOCOL.md` | Детали кадрирования вынесены в отдельный файл |
| `ARCHITECTURE.md` | `DESIGN_DECISIONS.md` | Обоснования выборов в отдельном файле |
| `requirements/*` | между собой | Три документа требований — единый контекст |
| `Defense_Etapy_1-2.md` | `Theory.md` | Защита опирается на теорию из того же раздела |
| `Defense_Etapy_1-2.md` | `TZ_Etapy_1-2.md` | На защите нужны ссылки на §§ ТЗ |
| `Defense_Etapy_1-2.md` | `src/*.rs` | Показываем конкретные файлы кода |
| `Theory.md` | `Defense_Etapy_1-2.md` | Теория → практическое применение на защите |
| все docs/ | `docs/README.md` | Обратная навигация к индексу из любого файла |
| все docs/ | `dev/CROSSLINKS.md` | Мета-ссылка: откуда брать актуальные ссылки |

---

## Как добавить новый файл

1. Создать файл в нужной папке
2. **Сюда** добавить строку в дерево и таблицу
3. Добавить nav-шапку в новый файл:
   ```markdown
   > **Навигация:** [docs/README.md](../README.md) · [CROSSLINKS](../dev/CROSSLINKS.md)
   ```
4. Обновить `docs/README.md` — добавить строку в нужную таблицу
5. При необходимости — обновить перекрёстные ссылки в смежных файлах

---

## Файлы вне docs/ с ссылками

| Файл | Ссылки на docs/ |
|------|-----------------|
| `README.md` (корень) | `docs/README.md`, `requirements/TZ_Etapy_1-2.md`, `requirements/Ustnie_Trebovaniya.md` |
| `experiments/scenarios/*.sh` | (нет документных ссылок — скрипты) |
| `src/` (Rust-файлы) | (ссылки через комментарии `//! §N ТЗ`) |
