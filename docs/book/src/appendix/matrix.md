# Матрица возможностей

Сводка того, что доступно в каком диалекте, с какой версии движка и при какой
Cargo-фиче. Источник — реализации capability-трейтов в
`crates/rurqe-postgres/src/dialect.rs` и `crates/rurqe-sqlite/src/dialect.rs`.

## 1. Маркеры версий

| Диалект | Маркеры | По умолчанию |
|---|---|---|
| PostgreSQL | `Pg14`, `Pg15`, `Pg16` | `Pg16` |
| SQLite | `Sqlite3_0`, `Sqlite3_24`, `Sqlite3_25`, `Sqlite3_30`, `Sqlite3_35`, `Sqlite3_38` | `Sqlite3_35` |

`Postgres<V: PgVersion = Pg16>` и `Sqlite<V: SqliteVersion = Sqlite3_35>` —
zero-sized параметризованные типы диалекта.

## 2. Capability-трейты

`✓` — трейт реализован, `—` — не реализован (использование не компилируется).

| Трейт | PG 14/15/16 | SQLite 3.0 | 3.24 | 3.25 | 3.30 | 3.35 | 3.38 |
|---|---|---|---|---|---|---|---|
| `SupportsReturning` | ✓ | — | — | — | — | ✓ | ✓ |
| `SupportsOnConflict` | ✓ | — | ✓ | ✓ | ✓ | ✓ | ✓ |
| `SupportsUpdateFrom` | ✓ | — | — | — | — | ✓ | ✓ |
| `SupportsAggregateFilter` | ✓ | — | — | — | ✓ | ✓ | ✓ |
| `SupportsWindowFunctions` | ✓ | — | — | ✓ | ✓ | ✓ | ✓ |
| `SupportsNullsOrdering` | ✓ | — | — | — | ✓ | ✓ | ✓ |
| `SupportsJsonAggregation` | ✓ | — | — | — | — | — | ✓ |
| `SupportsDistinctOn` | ✓ | — | — | — | — | — | — |
| `SupportsLateralJoin` | ✓ | — | — | — | — | — | — |
| `SupportsDeleteUsing` | ✓ | — | — | — | — | — | — |
| `SupportsFullOuterJoin` | ✓ | — | — | — | — | — | — |
| `SupportsRowLocking` | ✓ | — | — | — | — | — | — |
| `SupportsGreatest` | ✓ | — | — | — | — | — | — |
| `SupportsDistinctFrom` | ✓ | — | — | — | — | — | — |

Комментарии в коде фиксируют причины: `FULL OUTER JOIN` появился в SQLite 3.39.0
(за пределом самого свежего маркера), `GREATEST`/`LEAST` в SQLite не появились
вообще (их роль играют скалярные `max`/`min`), `IS DISTINCT FROM` — 3.39.0.

## 3. Поведенческие константы диалекта

| Константа | PostgreSQL | SQLite | Следствие |
|---|---|---|---|
| `SUPPORTS_ILIKE` | `true` (по умолчанию) | `false` | SQLite рендерит `LOWER(x) LIKE LOWER(?)`, а не `ILIKE` |
| `SUPPORTS_JSONB_ROW_AGG` | `true` | `false` | PostgreSQL делает `jsonb_agg(to_jsonb(...))`, SQLite собирает `json_group_array(json_object(...))` |

Капability `SupportsJsonAggregation` для SQLite начинается с 3.38.0 именно
потому, что до этого JSON1 был опциональной сборочной опцией: гейт по порогу
upsert’а компилировал бы оператор, который дефолтная сборка отвергает с
«no such function».

## 4. SQL ↔ диалект: что где рендерится

| Возможность | PostgreSQL | SQLite |
|---|---|---|
| Плейсхолдеры | `$1`, `$2`, … | `?` |
| Кавычки идентификаторов | `"x"`, `""` внутри | `"x"`, `""` внутри |
| `RETURNING` | всегда | с 3.35 |
| `ON CONFLICT` | всегда | с 3.24 |
| `UPDATE … FROM` | всегда | с 3.35 |
| `DELETE … USING` | всегда | нет |
| `LATERAL` | всегда | нет |
| `FULL OUTER JOIN` | всегда | нет |
| `FOR UPDATE` / `FOR SHARE` / `SKIP LOCKED` / `NOWAIT` | всегда | нет |
| `DISTINCT ON` | всегда | нет |
| `FILTER (WHERE …)` | всегда | с 3.30 |
| Оконные функции | всегда | с 3.25 |
| `NULLS FIRST/LAST` | всегда | с 3.30 |
| `GREATEST` / `LEAST` | всегда | нет |
| `IS [NOT] DISTINCT FROM` | всегда | нет |
| `ILIKE` | нативно | через `LOWER` |
| Вложенные коллекции (`nest_many`/`nest_one`) | `jsonb_agg` | `json_group_array` (с 3.38) |
| Массивы `T[]` | нативно | `Vec<T>` как JSON-документ |

## 5. Cargo-фичи фасада

| Фича | Что добавляет в `rurqe` |
|---|---|
| `postgres` | `PgConnection`, `PgError`; включает диалектную часть |
| `tokio-postgres` | `PgAsyncConnection`; включает диалектную часть |
| `deadpool-postgres` | `PgAsyncPool`; включает диалектную часть |
| `r2d2` | `PgPool`, `PgPoolError`, `SqlitePool`, `SqlitePoolError`, `PgConnection` |
| `sqlite` | `Sqlite`, `Sqlite3_*`, `SqliteConnection` |
| `macros` | `select!`, `cols!`, `dto!`, `sql!`, `raw_sql!`, `SqlType` |
| `migrate` | `rurqe::migrate` |
| `decimal` | `SqlVal::Decimal`, `MakeNullable` для `Decimal` |
| `uuid` | `SqlVal::Uuid` |
| `chrono` | `SqlVal::NaiveDateTime` / `DateTimeUtc` / `NaiveDate` |
| `serde_json` | `SqlVal::Json`, `Json<T>` |

Диалектная часть компилируется при **любом** PostgreSQL-адаптерном feature,
поэтому типизированный запрос можно собрать и отрендерить, не подключая драйвер:

```text
cargo build -p rurqe --no-default-features --features tokio-postgres
```

## 6. Значения ↔ Rust

| `SqlVal` | Rust | Фича |
|---|---|---|
| `Null` | `Option<T>::None` | — |
| `I32` | `i16`, `i32`, `u32`, `usize`, `bool`(0/1) | — |
| `I64` | `i64`, `u64`/`usize` в диапазоне | — |
| `F64` | `f64` | — |
| `Bool` | `bool` | — |
| `Text` | `String`, `&str` | — |
| `Bytes` | `Vec<u8>`, `&[u8]` | — |
| `Array` | `Vec<T>`, `T: SqlArrayElement` | — |
| `Decimal` | `rust_decimal::Decimal` | `decimal` |
| `Uuid` | `uuid::Uuid` | `uuid` |
| `NaiveDateTime` | `chrono::NaiveDateTime` | `chrono` |
| `DateTimeUtc` | `chrono::DateTime<Utc>` | `chrono` |
| `NaiveDate` | `chrono::NaiveDate` | `chrono` |
| `Json` | `serde_json::Value`, `Json<T>` | `serde_json` |

`SqlVal` помечен `#[non_exhaustive]`: набор вариантов зависит от фич, поэтому
`match` вне крейта обязан иметь wildcard-ветку.

## 7. Адаптеры и трейты исполнения

| Адаптер | Фича | Синхронный/асинхронный | Дополнительно |
|---|---|---|---|
| `PgConnection` | `postgres` / `r2d2` | sync | `TransactionBackend`, `TransactionalConnection` |
| `PgPool<M>` | `r2d2` | sync | — |
| `PgAsyncConnection` | `tokio-postgres` | async | `transaction()` → `PgAsyncTransaction` |
| `PgAsyncTransaction` | `tokio-postgres` | async | `commit()`, `rollback()` |
| `PgAsyncPool` | `deadpool-postgres` | async | — |
| `SqliteConnection` | `sqlite` | sync | `TransactionBackend`, `TransactionalConnection`, `Deref` к `rusqlite::Connection` |
| `SqlitePool<M>` | `r2d2` | sync | — |

## 8. Матрица «фича движка → доступный API»

| Нужно | PostgreSQL | SQLite |
|---|---|---|
| вернуть строки из `INSERT`/`UPDATE`/`DELETE` | `.returning(...)` | `.returning(...)` с `Sqlite3_35` |
| upsert | `.on_conflict(...)` | с `Sqlite3_24` |
| обновить из другой таблицы | `.from_table(...)` в `UPDATE` | с `Sqlite3_35` |
| удалить через другую таблицу | `.using(...)` | невозможно |
| блокировка строк | `.for_update()`, `.for_share()`, `.skip_locked()`, `.nowait()` | невозможно |
| оконные функции | `row_number()`, `rank()`, `lag`, `lead`, `… .over(spec)` | с `Sqlite3_25` |
| `FILTER (WHERE …)` на агрегате | да | с `Sqlite3_30` |
| `NULLS FIRST/LAST` | да | с `Sqlite3_30` |
| вложенные коллекции | `nest_many` / `nest_one` | с `Sqlite3_38` |
| массивы | `Vec<T>` нативно | `Vec<T>` как JSON |

Подробности по каждой возможности — в разделе
[Диалекты](../dialects/overview.md) и подглавах PostgreSQL и SQLite.
