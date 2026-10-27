# Capabilities и версионные пороги

SQLite наращивал SQL-поверхность десятилетиями, поэтому каждая возможность
диалекта доказана для конкретной версии движка. Порог выражается константными
дженериками:

```rust
pub trait SqliteVersion: 'static + Send + Sync {}
pub trait SqliteVersionAtLeast<const MAJOR: u32, const MINOR: u32>: SqliteVersion {}
```

`impl<V: SqliteVersion + SqliteVersionAtLeast<3, 35>> SupportsReturning for Sqlite<V> {}`
— маркер реализован только для версий, прошедших порог, поэтому метод, который
выпускает клаузу, для более старой версии просто не существует.

## Таблица порогов

| Возможность | Маркер | Порог | Объявленные версии, где есть |
|---|---|---|---|
| `ON CONFLICT` upsert | `SupportsOnConflict` | 3.24.0 | `Sqlite3_24`, `Sqlite3_25`, `Sqlite3_30`, `Sqlite3_35`, `Sqlite3_38` |
| окна `OVER (…)` | `SupportsWindowFunctions` | 3.25.0 | `Sqlite3_25`, `Sqlite3_30`, `Sqlite3_35`, `Sqlite3_38` |
| агрегатный `FILTER (WHERE …)` | `SupportsAggregateFilter` | 3.30.0 | `Sqlite3_30`, `Sqlite3_35`, `Sqlite3_38` |
| `NULLS FIRST` / `NULLS LAST` | `SupportsNullsOrdering` | 3.30.0 | `Sqlite3_30`, `Sqlite3_35`, `Sqlite3_38` |
| `RETURNING` | `SupportsReturning` | 3.35.0 | `Sqlite3_35`, `Sqlite3_38` |
| `UPDATE … FROM` | `SupportsUpdateFrom` | 3.35.0 | `Sqlite3_35`, `Sqlite3_38` |
| JSON-агрегация набора строк | `SupportsJsonAggregation` | 3.38.0 | `Sqlite3_38` |

`UPDATE … FROM` появился в движке в 3.33.0, но это ниже базовой версии
диалекта (`Sqlite3_35`), поэтому маркер начинается с ближайшего объявленного
порога.

JSON-функции (`json_object`, `json_group_array` и остальная семья) были
опциональным расширением сборки (`SQLITE_ENABLE_JSON1`) до 3.38.0, когда
вошли в сборку по умолчанию. Порог поэтому стоит на 3.38, а не на пороге
upsert: иначе statement компилировался бы, а сборка по умолчанию отвергала бы
его сообщением «no such function».

## Чего нет ни в одной версии

Следующие маркеры ядра не реализованы ни для одного объявленного маркера
SQLite:

| Маркер | Причина |
|---|---|
| `SupportsDistinctOn` | `DISTINCT ON` — расширение PostgreSQL |
| `SupportsLateralJoin` | в SQLite нет `LATERAL` |
| `SupportsDeleteUsing` | в SQLite нет `DELETE … USING` |
| `SupportsFullOuterJoin` | `FULL OUTER JOIN` появился в 3.39.0, дальше всех объявленных маркеров |
| `SupportsRowLocking` | SQLite блокирует файл базы, а не строки |
| `SupportsGreatest` | `GREATEST`/`LEAST` в движок не добавлены (их роль играют скалярные `max`/`min`) |
| `SupportsDistinctFrom` | `IS DISTINCT FROM` появился в 3.39.0, дальше всех объявленных маркеров |

Подробнее — [limitations](limitations.md).

## Чем это закреплено

Матрица держится не только документацией: `crates/rurqe-sqlite/src/dialect.rs`
содержит тесты

* `capabilities_are_gated_by_version` — `SupportsOnConflict`,
  `SupportsReturning`, `SupportsAggregateFilter`, `SupportsUpdateFrom` для
  версий выше порога и для `Sqlite` без аннотации;
* `expression_capabilities_are_gated_by_version` — `SupportsWindowFunctions`,
  `SupportsNullsOrdering`, `SupportsJsonAggregation`;
* `engine_flags_are_never_permissive_by_default` — `SUPPORTS_ILIKE` и
  `SUPPORTS_JSONB_ROW_AGG` выключены для каждого маркера версии.

Каждый тест компилируется только пока существуют соответствующие `impl`-блоки,
поэтому удаление возможности ломает сборку, а не запрос в продакшене.

## Preludes, привязанные к версии

Выбор версии делается один раз на модуль — через prelude нужной версии:

```rust
use rurqe::sqlite::sqlite3_24::prelude::*;
```

Модуль `sqlite3_24` видит только возможности 3.24: методов `RETURNING` в нём
нет вообще, потому что версия не проходит порог `SupportsReturning`. Полный
список — `rurqe::sqlite::sqlite3_0` … `rurqe::sqlite::sqlite3_38`; см.
[preludes](../preludes.md).

Импортируйте ровно один prelude на модуль: два диалекта в области видимости
делают точки входа неоднозначными, и компилятор скажет об этом на первом
вызове.
