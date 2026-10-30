# Dialects & Capabilities

RURQE targets two engines — PostgreSQL and SQLite — and treats each one as a dialect of the typed builder: a zero-sized type parameter that names the rules a statement is rendered with. A dialect adds no field, no allocation, and no runtime branch to a query. It decides how identifiers and parameters are spelled and which clauses the pinned engine release accepts, and both decisions are settled while the statement is assembled: a clause the target cannot parse cannot be named on the builder, so a mismatch surfaces as a compile error instead of a rejection from a running database.

## What the Dialect Decides

The same chain composes under either engine, and the dialect supplies the spelling:

| Rendering rule | PostgreSQL | SQLite |
|---|---|---|
| Identifier quoting | `"users"` | `"users"` |
| Bind placeholder | `$1`, `$2`, … | `?` |
| Case-insensitive `LIKE` | `ILIKE` | `LIKE` over `LOWER`-folded operands |
| Nested collection document | `jsonb_agg(to_jsonb(...))` | `json_group_array(json_object(...))` |

A statement is rendered into its SQL text together with its ordered parameter list, so nothing about the dialect survives into execution.

## Pinned Dialect Preludes

A prelude binds the four entry points — `select_from`, `insert_into`, `update`, and `delete_from` — to one dialect, and supplies the `Error` and `Result` aliases over that dialect's driver error. The PostgreSQL prelude pins the dialect to PostgreSQL 16:

```rust,no_run
{{#rustdoc_include ../examples/dialects.rs:postgres}}
```

```sql
SELECT "id" AS "user_id" FROM "users" WHERE ("id" = $1)
```

The SQLite prelude pins the dialect to SQLite 3.35, and the same chain renders the same statement with a positional placeholder:

```rust,no_run
{{#rustdoc_include ../examples/dialects.rs:sqlite}}
```

```sql
SELECT "id" AS "user_id" FROM "users" WHERE ("id" = ?)
```

## Pinned Releases

Both engines expose their releases as modules, and every module carries the same four entry points together with its own `prelude`:

- PostgreSQL: `pg14`, `pg15`, and `pg16`.
- SQLite: `sqlite3_0`, `sqlite3_24`, `sqlite3_25`, `sqlite3_30`, `sqlite3_35`, and `sqlite3_38`.

```rust,no_run
{{#rustdoc_include ../examples/dialects.rs:pg_versions}}
```

```rust,no_run
{{#rustdoc_include ../examples/dialects.rs:version}}
```

The marker pins the capability set, never the builder: `pg14` and `pg16` write the statement quoted above without a difference, and `sqlite3_0` writes it with the positional placeholder. The marker states what a deployment promises about its engine; the connected database is never probed at runtime.

## Capability Summary

Every clause is either implemented by a dialect or absent from it. The table states whether PostgreSQL accepts the clause and which SQLite release introduced it; a clause absent from a target has no builder method there at all.

| Clause | PostgreSQL | SQLite |
|---|---|---|
| `RETURNING` | Supported | From 3.35 (`sqlite3_35`) |
| `ON CONFLICT` (`DO NOTHING`, `DO UPDATE`) | Supported | From 3.24 (`sqlite3_24`) |
| `UPDATE ... FROM` | Supported | From 3.35 (`sqlite3_35`) |
| `DELETE ... USING` | Supported | Unavailable |
| `DISTINCT ON` | Supported | Unavailable |
| `LATERAL` joins | Supported | Unavailable |
| `FULL OUTER JOIN` | Supported | Unavailable |
| Row locking (`FOR UPDATE`, `FOR SHARE`, `SKIP LOCKED`, `NOWAIT`) | Supported | Unavailable |
| Window functions (`OVER (...)`) | Supported | From 3.25 (`sqlite3_25`) |
| Aggregate `FILTER (WHERE ...)` | Supported | From 3.30 (`sqlite3_30`) |
| `NULLS FIRST` / `NULLS LAST` | Supported | From 3.30 (`sqlite3_30`) |
| `GREATEST` / `LEAST` | Supported | Unavailable |
| `IS DISTINCT FROM` / `IS NOT DISTINCT FROM` | Supported | Unavailable |
| Nested collections (`nest_many`, `nest_one`) | Supported | From 3.38 (`sqlite3_38`) |

## PostgreSQL Extensions

PostgreSQL ships expressions the embedded engine does not have, and each one is an ordinary method on the typed builder under the PostgreSQL prelude.

A JSON or JSONB column is filtered and projected through the native operators, so a document never leaves the typed builder:

```rust,no_run
{{#rustdoc_include ../examples/dialects.rs:jsonb}}
```

```sql
SELECT "id" AS "profile_id", ("metadata" ->> $1) AS "owner" FROM "profiles" WHERE ("metadata" ? $2)
```

An array column compares through the native containment operators, and the element type follows the column:

```rust,no_run
{{#rustdoc_include ../examples/dialects.rs:arrays}}
```

```sql
SELECT "id" AS "profile_id" FROM "profiles" WHERE ("tags" @> $1)
```

A text column matches a POSIX regular expression:

```rust,no_run
{{#rustdoc_include ../examples/dialects.rs:regex}}
```

```sql
SELECT "id" AS "profile_id" FROM "profiles" WHERE ("bio" ~ $1)
```

A query claims the rows it selects until the transaction ends, and `SKIP LOCKED` passes over the rows another transaction already holds:

```rust,no_run
{{#rustdoc_include ../examples/dialects.rs:for_update}}
```

```sql
SELECT "id" AS "user_id" FROM "users" WHERE ("id" = $1) FOR UPDATE SKIP LOCKED
```

## SQLite Release Thresholds

SQLite gained its clauses across releases, so the release marker a statement is pinned to decides which of them exist. An upsert is reachable from the module pinned to the release that introduced it:

```rust,no_run
{{#rustdoc_include ../examples/dialects.rs:sqlite_upsert}}
```

`RETURNING` needs a later release, and the module pinned to it is where the clause becomes reachable:

```rust,no_run
{{#rustdoc_include ../examples/dialects.rs:sqlite_returning}}
```

Below those thresholds the clause has no builder method, so the statement is rejected by the compiler rather than by the engine:

```rust,compile_fail
use rurqe::sqlite::sqlite3_0::prelude::*;

table! {
    User => "users" {
        UserId => "id": i64,
    }
}

// Rejected at compile time: RETURNING requires SQLite 3.35+
let _ = delete_from(User).allow_all().returning(cols!(UserId));
```

## Runtime Independence

Assembling a statement performs no I/O and carries no runtime: the chain that introduces the table, filters the rows, and projects the columns is identical for every connection type, and the runtime appears only in the terminal call. Each engine ships a blocking adapter, and PostgreSQL adds an asynchronous one:

| Engine | Blocking adapter | Asynchronous adapter |
|---|---|---|
| PostgreSQL | `PgConnection` | `PgAsyncConnection` |
| SQLite | `SqliteConnection` | — |

An asynchronous connection returns a future consumed with `.await?`, while a blocking connection returns the same `Result` directly.

## Embedded SQLite

SQLite is the embedded engine: the adapter runs over a synchronous connection, and a private in-memory database is what a test suite or a local deployment opens.

```rust,no_run
{{#rustdoc_include ../examples/sync_execution.rs:open_in_memory}}
```

Every terminal call on that connection returns its result directly instead of a future.
