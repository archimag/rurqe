# Capability matrix

A summary of what is available in which dialect, from which engine version, and
under which Cargo feature. The source is the capability-trait implementations in
`crates/rurqe-postgres/src/dialect.rs` and `crates/rurqe-sqlite/src/dialect.rs`.

## 1. Version markers

| Dialect | Markers | Default |
|---|---|---|
| PostgreSQL | `Pg14`, `Pg15`, `Pg16` | `Pg16` |
| SQLite | `Sqlite3_0`, `Sqlite3_24`, `Sqlite3_25`, `Sqlite3_30`, `Sqlite3_35`, `Sqlite3_38` | `Sqlite3_35` |

`Postgres<V: PgVersion = Pg16>` and `Sqlite<V: SqliteVersion = Sqlite3_35>` are
zero-sized parameterized dialect types.

## 2. Capability traits

`✓` — the trait is implemented, `—` — it is not (using it does not compile).

| Trait | PG 14/15/16 | SQLite 3.0 | 3.24 | 3.25 | 3.30 | 3.35 | 3.38 |
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

Code comments record the reasons: `FULL OUTER JOIN` arrived in SQLite 3.39.0
(beyond the newest marker), `GREATEST`/`LEAST` never arrived in SQLite (the
scalar `max`/`min` play their role), and `IS DISTINCT FROM` — 3.39.0.

## 3. Dialect behaviour constants

| Constant | PostgreSQL | SQLite | Consequence |
|---|---|---|---|
| `SUPPORTS_ILIKE` | `true` (default) | `false` | SQLite renders `LOWER(x) LIKE LOWER(?)` instead of `ILIKE` |
| `SUPPORTS_JSONB_ROW_AGG` | `true` | `false` | PostgreSQL emits `jsonb_agg(to_jsonb(...))`, SQLite assembles `json_group_array(json_object(...))` |

The `SupportsJsonAggregation` capability for SQLite starts at 3.38.0 precisely
because before that JSON1 was an optional build-time option: a gate at the
upsert threshold would compile a statement that a default build rejects with
"no such function".

## 4. SQL ↔ dialect: what renders where

| Capability | PostgreSQL | SQLite |
|---|---|---|
| Placeholders | `$1`, `$2`, … | `?` |
| Identifier quotes | `"x"`, with `""` inside | `"x"`, with `""` inside |
| `RETURNING` | always | from 3.35 |
| `ON CONFLICT` | always | from 3.24 |
| `UPDATE … FROM` | always | from 3.35 |
| `DELETE … USING` | always | never |
| `LATERAL` | always | never |
| `FULL OUTER JOIN` | always | never |
| `FOR UPDATE` / `FOR SHARE` / `SKIP LOCKED` / `NOWAIT` | always | never |
| `DISTINCT ON` | always | never |
| `FILTER (WHERE …)` | always | from 3.30 |
| Window functions | always | from 3.25 |
| `NULLS FIRST/LAST` | always | from 3.30 |
| `GREATEST` / `LEAST` | always | never |
| `IS [NOT] DISTINCT FROM` | always | never |
| `ILIKE` | native | through `LOWER` |
| Nested collections (`nest_many`/`nest_one`) | `jsonb_agg` | `json_group_array` (from 3.38) |
| Arrays `T[]` | native | `Vec<T>` as a JSON document |

## 5. Facade Cargo features

| Feature | What it adds to `rurqe` |
|---|---|
| `postgres` | `PgConnection`, `PgError`; enables the dialect part |
| `tokio-postgres` | `PgAsyncConnection`; enables the dialect part |
| `deadpool-postgres` | `PgAsyncPool`; enables the dialect part |
| `r2d2` | `PgPool`, `PgPoolError`, `SqlitePool`, `SqlitePoolError`, `PgConnection` |
| `sqlite` | `Sqlite`, `Sqlite3_*`, `SqliteConnection` |
| `macros` | `select!`, `cols!`, `dto!`, `sql!`, `raw_sql!`, `SqlType` |
| `migrate` | `rurqe::migrate` |
| `decimal` | `SqlVal::Decimal`, `MakeNullable` for `Decimal` |
| `uuid` | `SqlVal::Uuid` |
| `chrono` | `SqlVal::NaiveDateTime` / `DateTimeUtc` / `NaiveDate` |
| `serde_json` | `SqlVal::Json`, `Json<T>` |

The dialect part compiles under **any** PostgreSQL adapter feature, so a typed
query can be composed and rendered without pulling a driver:

```text
cargo build -p rurqe --no-default-features --features tokio-postgres
```

## 6. Values ↔ Rust

| `SqlVal` | Rust | Feature |
|---|---|---|
| `Null` | `Option<T>::None` | — |
| `I32` | `i16`, `i32`, `u32`, `usize`, `bool`(0/1) | — |
| `I64` | `i64`, `u64`/`usize` within range | — |
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

`SqlVal` is marked `#[non_exhaustive]`: the set of variants depends on the
features, so a `match` outside the crate must carry a wildcard arm.

## 7. Adapters and execution traits

| Adapter | Feature | Sync/async | Additional traits |
|---|---|---|---|
| `PgConnection` | `postgres` / `r2d2` | sync | `TransactionBackend`, `TransactionalConnection` |
| `PgPool<M>` | `r2d2` | sync | — |
| `PgAsyncConnection` | `tokio-postgres` | async | `transaction()` → `PgAsyncTransaction` |
| `PgAsyncTransaction` | `tokio-postgres` | async | `commit()`, `rollback()` |
| `PgAsyncPool` | `deadpool-postgres` | async | — |
| `SqliteConnection` | `sqlite` | sync | `TransactionBackend`, `TransactionalConnection`, `Deref` to `rusqlite::Connection` |
| `SqlitePool<M>` | `r2d2` | sync | — |

## 8. Matrix: engine feature → available API

| Need | PostgreSQL | SQLite |
|---|---|---|
| return rows from `INSERT`/`UPDATE`/`DELETE` | `.returning(...)` | `.returning(...)` from `Sqlite3_35` |
| upsert | `.on_conflict(...)` | from `Sqlite3_24` |
| update from another table | `.from_table(...)` in `UPDATE` | from `Sqlite3_35` |
| delete through another table | `.using(...)` | impossible |
| row locking | `.for_update()`, `.for_share()`, `.skip_locked()`, `.nowait()` | impossible |
| window functions | `row_number()`, `rank()`, `lag`, `lead`, `… .over(spec)` | from `Sqlite3_25` |
| `FILTER (WHERE …)` on an aggregate | yes | from `Sqlite3_30` |
| `NULLS FIRST/LAST` | yes | from `Sqlite3_30` |
| nested collections | `nest_many` / `nest_one` | from `Sqlite3_38` |
| arrays | `Vec<T>` natively | `Vec<T>` as JSON |

Details on each capability live in the [Dialects](../dialects/overview.md)
section and its PostgreSQL and SQLite chapters.
