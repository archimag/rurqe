# Comparing PostgreSQL and SQLite

RURQE targets two engines with a single builder: PostgreSQL as a client-server database and SQLite as an embedded one. The dialect is a compile-time parameter of a statement — it decides how identifiers and parameters are spelled and which clauses the pinned engine release can parse — and nothing about it survives into execution. The same chain therefore composes against either engine, and only the terminal call names the destination.

This chapter compares the two engines where they genuinely diverge: the deployment model each one assumes, the text a statement renders to, the expressions only one of them implements, and the release thresholds that gate a clause inside SQLite.

## Two Deployment Models

The deployment model decides what the engine guarantees about concurrency, runtime, and lifecycle, never how a query is typed.

- **Concurrency.** PostgreSQL isolates concurrent writers per row: a transaction claims only the rows it modifies, so other transactions keep writing the rest of the table. SQLite serialises writers through the database file: a write takes the whole file, and no statement claims an individual row.
- **Runtime.** PostgreSQL ships a blocking adapter and an asynchronous one, so one statement executes on a worker thread or on an asynchronous executor. SQLite ships a blocking adapter only, because an embedded statement completes inside the calling process.
- **Lifecycle.** A PostgreSQL database is a long-lived deployment target that exists before the application connects and outlives it. A SQLite database is created on demand as a file, or as a private in-memory image that disappears together with the connection.

An in-memory database is what a test suite or a local deployment opens:

```rust,no_run
{{#rustdoc_include ../../examples/sync_execution.rs:open_in_memory}}
```

## How a Statement Is Rendered

Both dialects quote an identifier with double quotes and double every embedded quote, so a reserved word or a mixed-case name needs no attention from the caller. The bind parameter is where the two part: PostgreSQL numbers its parameters (`$1`, `$2`, …), while SQLite writes an anonymous `?` for each one. The parameter list is ordered either way, so the difference stays inside the statement text.

The same chain composes under both preludes:

```rust,no_run
{{#rustdoc_include ../../examples/dialects.rs:postgres}}
```

```sql
SELECT "id" AS "user_id" FROM "users" WHERE ("id" = $1)
```

```rust,no_run
{{#rustdoc_include ../../examples/dialects.rs:sqlite}}
```

```sql
SELECT "id" AS "user_id" FROM "users" WHERE ("id" = ?)
```

The remaining rendering rules follow the same split:

| Rendering rule | PostgreSQL | SQLite |
|---|---|---|
| Identifier quoting | `"users"` | `"users"` |
| Bind placeholder | `$1`, `$2`, … | `?` |
| Case-insensitive matching | `ILIKE` | `LIKE` over `LOWER`-folded operands |
| Nested collection document | `jsonb_agg(to_jsonb(...))` | `json_group_array(json_object(...))` |

## Capability Matrix

Every clause is either implemented by a dialect or absent from it, and inside SQLite a clause is further gated by the pinned release. A clause absent from the target has no builder method there at all, so the statement is rejected by the compiler instead of by the engine.

| Clause or feature | PostgreSQL | SQLite |
|---|---|---|
| `RETURNING` | Supported | From 3.35 (`sqlite3_35`) |
| `ON CONFLICT` (`DO NOTHING`, `DO UPDATE`) | Supported | From 3.24 (`sqlite3_24`) |
| `UPDATE ... FROM` | Supported | From 3.35 (`sqlite3_35`) |
| `DELETE ... USING` | Supported | Unavailable |
| `DISTINCT ON` | Supported | Unavailable |
| `LATERAL` joins | Supported | Unavailable |
| `FULL OUTER JOIN` | Supported | Unavailable |
| Row locking (`FOR UPDATE`, `FOR SHARE`, `NOWAIT`, `SKIP LOCKED`) | Supported | Unavailable |
| Window functions (`OVER (...)`) | Supported | From 3.25 (`sqlite3_25`) |
| Aggregate `FILTER (WHERE ...)` | Supported | From 3.30 (`sqlite3_30`) |
| `NULLS FIRST` / `NULLS LAST` | Supported | From 3.30 (`sqlite3_30`) |
| `GREATEST` / `LEAST` | Supported | Unavailable |
| `IS DISTINCT FROM` / `IS NOT DISTINCT FROM` | Supported | Unavailable |
| Nested collections (`nest_many`, `nest_one`) | Supported | From 3.38 (`sqlite3_38`) |
| JSON document operators (`@>`, `?`, `->`, `->>`) | Supported | Unavailable |
| Array columns (`T[]`, `@>`, `<@`, `&&`, `ANY`, `ALL`) | Supported | Unavailable |
| Regular expressions (`~`, `~*`, `!~`, `!~*`) | Supported | Unavailable |
| Full-text search (`@@`, `to_tsvector`, `to_tsquery`) | Supported | Unavailable |

## Release Markers Decide the Reachable Clause Set

Both engines expose their releases as modules, and every module carries the same entry points with the dialect pinned to that release. Pinning decides which clauses a chain can name, never how the named clauses render.

```rust,no_run
{{#rustdoc_include ../../examples/dialects.rs:pg_versions}}
```

Both pinned releases render the statement written above, because every clause it names belongs to the oldest pinned release as well.

SQLite gained its clauses across releases, so the pinned release — never the connected database — decides which of them exist. The baseline module reaches the statement surface every release shares:

```rust,no_run
{{#rustdoc_include ../../examples/dialects.rs:version}}
```

```sql
SELECT "id" AS "user_id" FROM "users" WHERE ("id" = ?)
```

A clause introduced after the pinned release has no builder method at that module, so the statement is rejected by the compiler instead of by the engine:

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

## Text Matching

A case-insensitive predicate is written once and keeps its meaning on both engines: PostgreSQL renders `ILIKE`, and SQLite, which has no such keyword, folds both operands with `LOWER`. The fold covers ASCII letters and keeps the comparison independent of the database collation.

PostgreSQL additionally matches a POSIX regular expression, and the pattern binds as one text parameter:

```rust,no_run
{{#rustdoc_include ../../examples/dialects.rs:regex}}
```

```sql
SELECT "id" AS "profile_id" FROM "profiles" WHERE ("bio" ~ $1)
```

The negated forms render `!~` and `!~*`, and a full-text predicate renders `("bio" @@ to_tsquery($1))` against a query built by `to_tsquery` or `plainto_tsquery`. SQLite implements neither operator, so the methods are absent from that dialect.

## JSON Documents and Array Columns

PostgreSQL models a document and an array as native column types with operators of their own. SQLite stores a document as JSON text and has no array type at all.

A JSON or JSONB column filters and projects through the native operators, so a document never leaves the typed builder:

```rust,no_run
{{#rustdoc_include ../../examples/dialects.rs:jsonb}}
```

```sql
SELECT "id" AS "profile_id", ("metadata" ->> $1) AS "owner" FROM "profiles" WHERE ("metadata" ? $2)
```

An array column compares through containment and overlap, and the element type follows the column:

```rust,no_run
{{#rustdoc_include ../../examples/dialects.rs:arrays}}
```

```sql
SELECT "id" AS "profile_id" FROM "profiles" WHERE ("tags" @> $1)
```

SQLite evaluates a JSON document with its own JSON functions, which is how a nested collection is assembled on that engine, but it offers neither containment nor extraction as builder methods, and a column holding several values has no typed representation.

## Row Locking

Inside a transaction, a PostgreSQL statement claims the rows it selects and holds them until the transaction ends. The wait modifier decides what happens when another transaction already holds a conflicting lock:

```rust,no_run
{{#rustdoc_include ../../examples/dialects.rs:for_update}}
```

```sql
SELECT "id" AS "user_id" FROM "users" WHERE ("id" = $1) FOR UPDATE SKIP LOCKED
```

`for_share` takes the weaker claim, which lets other transactions keep reading the rows, while `.nowait()` and `.skip_locked()` decide whether a statement waits for a lock another transaction holds.

SQLite locks the database file for the duration of a write instead of claiming individual rows, so it has no per-row equivalent and the locking methods do not exist on that dialect:

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
    .for_update();
```
