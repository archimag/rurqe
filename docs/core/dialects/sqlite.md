# SQLite

SQLite is the embedded engine: the database lives in a single file or in memory, and statements execute over a synchronous connection owned by the process. The dialect quotes identifiers with double quotes, formats positional `?` placeholders, and gates every clause behind the release that introduced it, so the pinned release — not the connected database — decides which clauses a statement may name.

## Pinned Releases

The engine releases are exposed as modules, and each one carries the builder entry points pinned to that release:

- `sqlite3_0`
- `sqlite3_24`
- `sqlite3_25`
- `sqlite3_30`
- `sqlite3_35`
- `sqlite3_38`

The unversioned prelude binds the dialect to SQLite 3.35:

```rust,no_run
{{#rustdoc_include ../../examples/dialects.rs:sqlite}}
```

```sql
SELECT "id" AS "user_id" FROM "users" WHERE ("id" = ?)
```

A pinned module records the release the statement targets. The same chain composes for any of them, and only the clauses that release does not support stay unreachable:

```rust,no_run
{{#rustdoc_include ../../examples/dialects.rs:version}}
```

```sql
SELECT "id" AS "user_id" FROM "users" WHERE ("id" = ?)
```

Both statements render the same text, because the pinned release decides which clauses the chain can name rather than how the named clauses render. The prelude also binds the `Error` and `Result` aliases to the driver error of the engine, so a terminal call propagates its failure with `?`. The marker states what a deployment promises about its engine; the connected database is never probed at runtime.

## Release Grid

Every module inherits the clauses of the releases before it and adds those of its own release:

| Release module | Clauses it adds |
|---|---|
| `sqlite3_0` | the baseline statement surface: `SELECT`, `INSERT`, `UPDATE`, `DELETE`, joins, grouping, ordering, and set operations |
| `sqlite3_24` | `ON CONFLICT` upserts |
| `sqlite3_25` | window functions (`OVER (...)`) |
| `sqlite3_30` | aggregate `FILTER (WHERE ...)` and explicit null ordering |
| `sqlite3_35` | `RETURNING` and `UPDATE ... FROM` |
| `sqlite3_38` | nested collections over the JSON functions |

## Capability Summary

A clause or expression is either implemented by some release of the engine or absent from the dialect entirely. A clause absent from the target has no builder method there at all, so the statement is rejected by the compiler rather than by the engine.

| Clause or feature | SQLite |
|---|---|
| `ON CONFLICT` (`DO NOTHING`, `DO UPDATE`) | From 3.24 (`sqlite3_24`) |
| Window functions (`OVER (...)`) | From 3.25 (`sqlite3_25`) |
| Aggregate `FILTER (WHERE ...)` | From 3.30 (`sqlite3_30`) |
| `NULLS FIRST` / `NULLS LAST` | From 3.30 (`sqlite3_30`) |
| `RETURNING` | From 3.35 (`sqlite3_35`) |
| `UPDATE ... FROM` | From 3.35 (`sqlite3_35`) |
| Nested collections (`nest_many`, `nest_one`) | From 3.38 (`sqlite3_38`) |
| Row locking (`FOR UPDATE`, `FOR SHARE`, `NOWAIT`, `SKIP LOCKED`) | Unavailable |
| `DISTINCT ON` | Unavailable |
| `LATERAL` joins | Unavailable |
| `FULL OUTER JOIN` | Unavailable |
| `DELETE ... USING` | Unavailable |
| `GREATEST` / `LEAST` | Unavailable |
| `IS DISTINCT FROM` / `IS NOT DISTINCT FROM` | Unavailable |
| JSON document operators (`@>`, `?`, `->`, `->>`) | Unavailable |
| Array columns (`T[]`) | Unavailable |
| Regular expressions (`~`, `~*`, `!~`, `!~*`) | Unavailable |
| Full-text search (`@@`, `to_tsvector`, `to_tsquery`) | Unavailable |

## Rendering

- An identifier is double-quoted, and an embedded double quote is doubled: `"user""name"`.
- A bind parameter is an anonymous `?`, so the parameter list follows the statement text in order instead of addressing parameters by number. A nested statement keeps that ordering as well: its parameters continue the list of the statement that projects it, so a filter written inside a nested collection and one written in the enclosing statement never collide.
- The engine has no `ILIKE` operator: a case-insensitive predicate renders as `LOWER(...) LIKE LOWER(...)` over both operands, which folds ASCII letters and keeps the semantics independent of the database collation.

## Upsert with `ON CONFLICT`

A conflicting row is skipped or rewritten from the module pinned to the release that introduced the clause:

```rust,no_run
{{#rustdoc_include ../../examples/dialects.rs:sqlite_upsert}}
```

```sql
INSERT INTO "users" ("id", "name") VALUES (?, ?) ON CONFLICT DO NOTHING
```

The clause binds after the inserted values and before any `RETURNING` projection, so its parameters follow the row parameters in statement order. Below the threshold the clause has no builder method:

```rust,compile_fail
use rurqe::sqlite::sqlite3_0::prelude::*;

table!(
    User => "users" {
        UserId => "id": i64,
    }
);

// Rejected at compile time: ON CONFLICT requires SQLite 3.24+.
let _ = insert_into(User)
    .values(UserId::set(1i64))
    .on_conflict_do_nothing();
```

## `RETURNING`

`RETURNING` arrived in SQLite 3.35, and the module pinned to that release is where the clause becomes reachable:

```rust,no_run
{{#rustdoc_include ../../examples/dialects.rs:sqlite_returning}}
```

```sql
INSERT INTO "users" ("id", "name") VALUES (?, ?) RETURNING "id" AS "user_id"
```

The clause hands the affected rows back in the same statement, so a mutation returns its decoded row without a second round trip. Below the threshold the clause has no builder method:

```rust,compile_fail
use rurqe::sqlite::sqlite3_24::prelude::*;

table!(
    User => "users" {
        UserId => "id": i64,
    }
);

// Rejected at compile time: RETURNING requires SQLite 3.35+.
let _ = delete_from(User).allow_all().returning(cols!(UserId));
```

## Nested Collections and JSON Aggregation

A nested collection is assembled from the JSON functions: every child row becomes a `json_object`, and the objects are collected with `json_group_array`. The extension carrying those functions was an opt-in build option until SQLite 3.38, which is why the projection is reachable only from the module pinned to that release:

```rust,no_run
{{#rustdoc_include ../../examples/nested.rs:collection_sqlite}}
```

```sql
SELECT "id",
       COALESCE((SELECT json_group_array(json_object('order_id', "__rurqe_sub"."order_id",
                                                     'total', "__rurqe_sub"."total"))
                 FROM (SELECT "id" AS "order_id", "total" AS "total" FROM "orders")
                      AS "__rurqe_sub"), '[]')
FROM "users"
```

The document holds one object per child row, keyed by the field names of the nested projection, so the row decodes without a positional index, and a parent without children receives an empty array instead of a missing field. Below the threshold the projection is rejected by the compiler:

```rust,compile_fail
use rurqe::sqlite::sqlite3_35::prelude::*;

table!(
    User => "users" {
        UserId => "id": i64,
    }
);

table!(
    Order => "orders" {
        OrderId => "id": i64,
        OrderUserId => "user_id": i64,
    }
);

// Rejected at compile time: the JSON functions a nested collection renders with
// are part of a default build only from SQLite 3.38, so the projection is
// rejected below that release.
let _ = select_from(User)
    .select(cols! {
        user_id: UserId,
        orders: nest_many(select_from(Order).select(cols!(order_id: OrderId))),
    });
```

## Embedded and In-Memory Databases

The adapter runs over a synchronous connection, and a private in-memory database is what a test suite or a local deployment opens:

```rust,no_run
{{#rustdoc_include ../../examples/sync_execution.rs:open_in_memory}}
```

The contents of that database disappear together with the connection. A file-backed deployment wraps a connection opened on a file path instead, and the adapter accepts it through `SqliteConnection::new`, so one statement surface serves both forms. The adapter also dereferences to the underlying driver connection, which is how a statement the typed builder does not model is issued directly. A raw statement behaves exactly as the driver defines it and takes no part in the bookkeeping of the typed transaction around it.

## Synchronous Execution

SQLite has no asynchronous adapter: every terminal call returns its result directly, and the chain that assembles the statement is identical to the asynchronous one:

```rust,no_run
{{#rustdoc_include ../../examples/sync_execution.rs:load_user}}
```

```sql
SELECT "id" AS "user_id" FROM "users"
```

A pooled deployment enables the `r2d2` feature and executes through `SqlitePool`, which implements the same execution contract: the pool checks a connection out, runs the statement, and returns the connection to the pool, while the builder chain and the terminal call stay unchanged.

A transaction block runs between `BEGIN` and `COMMIT`: the block commits when it returns `Ok`, and it rolls back when it returns an error or panics, so no partial work is left behind. A block opened inside another one becomes a `SAVEPOINT`, so an inner failure rolls back only its own statements while the enclosing block stays intact. The block takes no isolation level: SQLite serialises access through database-level locking, so a level would have no `BEGIN` clause to map onto.

## Server-Only Clauses

Some clauses belong to a client-server engine and have no meaning in an embedded database. SQLite locks the whole database file for the duration of a write, so it has no per-row locking, and the locking methods are absent from the builder:

```rust,compile_fail
use rurqe::sqlite::prelude::*;

table!(
    Job => "jobs" {
        JobId => "id": i64,
    }
);

// Rejected at compile time: SQLite has no per-row locking, so the clause has
// no builder method.
let _ = select_from(Job)
    .select(cols!(JobId))
    .for_update()
    .skip_locked();
```

`DISTINCT ON` is a PostgreSQL clause as well, so the builder offers no such method on this dialect:

```rust,compile_fail
use rurqe::sqlite::prelude::*;

table!(
    User => "users" {
        UserId => "id": i64,
    }
);

// Rejected at compile time: DISTINCT ON is not a clause SQLite implements.
let _ = select_from(User).select(cols!(UserId)).distinct_on(UserId);
```

The same holds for `LATERAL` joins, `FULL OUTER JOIN`, `DELETE ... USING`, `GREATEST` / `LEAST`, the null-safe `IS DISTINCT FROM` comparisons, the JSON document operators, array columns, regular expressions, and full-text search; the capability summary above records each of them as unavailable.
