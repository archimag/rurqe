# Summary

# Часть I. Введение

- [Философия RURQE](intro/philosophy.md)
- [Архитектурный тетраэдр](intro/tetrad.md)
- [Two-Tier SQL Correctness](intro/two_tier.md)
- [Quickstart за 5 минут](intro/quickstart.md)

# Часть II. Реляционная теория в типах Rust

- [Обзор части](theory/overview.md)
- [От реляционной алгебры Кодда к системе типов Rust](theory/codd_to_rust.md)
- [Области видимости: TableScope и ExprScope](theory/scopes.md)
- [Nullability: внешние соединения как изменение типа](theory/nullability.md)
- [Disjointness: уникальность имён отношений](theory/disjointness.md)
- [NF² и MULTISET: вложенные отношения](theory/nf2_multiset.md)

# Часть III. Диалекты

- [Обзор диалектов](dialects/overview.md)
- [Философия: native or nothing](dialects/philosophy.md)
- [Capability markers](dialects/capabilities.md)
- [Версии движков](dialects/versions.md)
- [Preludes](dialects/preludes.md)
- [PostgreSQL](dialects/postgres/overview.md)
  - [Capabilities](dialects/postgres/capabilities.md)
  - [DML: RETURNING, ON CONFLICT](dialects/postgres/dml.md)
  - [Блокировки строк](dialects/postgres/locking.md)
  - [LATERAL joins](dialects/postgres/lateral.md)
  - [JSON и JSONB](dialects/postgres/json.md)
  - [Массивы](dialects/postgres/arrays.md)
  - [Full-text search](dialects/postgres/fts.md)
  - [Регулярные выражения](dialects/postgres/regex.md)
  - [Enums](dialects/postgres/enums.md)
  - [Драйверы](dialects/postgres/drivers.md)
- [SQLite](dialects/sqlite/overview.md)
  - [Capabilities и версионные пороги](dialects/sqlite/capabilities.md)
  - [Upsert: ON CONFLICT](dialects/sqlite/upsert.md)
  - [Агрегаты, окна, сортировка](dialects/sqlite/filter.md)
  - [RETURNING и UPDATE … FROM](dialects/sqlite/returning.md)
  - [Ограничения без эмуляции](dialects/sqlite/limitations.md)
  - [JSON и вложенные коллекции](dialects/sqlite/json.md)
  - [Enums через CHECK](dialects/sqlite/enums.md)
  - [Драйвер rusqlite](dialects/sqlite/driver.md)
- [Конкурентность миграций](dialects/sqlite/concurrency.md)

# Часть IV. Вложенные результаты и MULTISET (NF²)

- [Вложенные результаты: `nest_many` и `nest_one`](features/nested.md)
- [MULTISET: как вложенность превращается в SQL](features/multiset.md)
- [Движки, capability-гейты и декодирование](features/multiset_engines.md)

# Часть V. Эволюция схемы и миграции

- [Эволюция схемы: обзор](migrations/overview.md)
- [Кризис традиционных миграций в эпоху AI](migrations/problem.md)
- [Два режима и критерии выбора](migrations/modes.md)
- [Традиционный путь: `V...` и `R...`](migrations/traditional.md)
- [Stage + Tail: декларативный stage и cold start](migrations/stage_tail.md)
- [The Golden Equation](migrations/golden_equation.md)
- [Безопасный прунинг истории](migrations/pruning.md)
- [Протокол чекпоинтов](migrations/checkpoint_protocol.md)

# Часть VI. Рантайм и типы

- [Рантайм: обзор](runtime/overview.md)
- [Транзакции и savepoints](runtime/transactions.md)
- [Пулы соединений](runtime/pools.md)
- [Экосистемные типы, значения и декодирование](runtime/types.md)
- [Пользовательские типы](runtime/custom_types.md)

# Приложения

- [Приложения](appendix/overview.md)
- [LLM Cheatsheet / Prompt Pack](appendix/llm_cheatsheet.md)
- [Справочник ошибок](appendix/compiler_errors.md)
- [Матрица возможностей](appendix/matrix.md)
