# Dialects & Capabilities

RURQE supports multiple database engines through zero-cost dialect types, version thresholds, and compile-time capability gates. Dialects customize identifier quoting, parameter placeholder formatting, and available SQL clauses without adding runtime checks.

## Pinned Dialect Preludes

Application code binds a dialect by importing its prelude. The prelude supplies builder entry points (`select_from`, `insert_into`, `update`, `delete_from`) and macro helpers pre-configured for the target engine, eliminating explicit generic parameters:

```rust,no_run
{{#rustdoc_include ../examples/dialects.rs:postgres}}
```

The PostgreSQL dialect quotes identifiers with double quotes (`"users"`) and formats numbered parameter placeholders (`$1, $2, ...`):

```sql
SELECT "id" AS "user_id" FROM "users" WHERE ("id" = $1)
```

The SQLite dialect generates double-quoted identifiers and positional parameter placeholders (`?`):

```rust,no_run
{{#rustdoc_include ../examples/dialects.rs:sqlite}}
```

```sql
SELECT "id" AS "user_id" FROM "users" WHERE ("id" = ?)
```

## Version-Pinned Preludes

Both engines expose their releases as modules, and every module carries the same builder entry points pinned to one engine version:

- PostgreSQL: `pg14`, `pg15`, and `pg16`, while the dialect itself defaults to PostgreSQL 16.
- SQLite: `sqlite3_0`, `sqlite3_24`, `sqlite3_25`, `sqlite3_30`, `sqlite3_35`, and `sqlite3_38`, while the dialect itself defaults to SQLite 3.35.

```rust,no_run
{{#rustdoc_include ../examples/dialects.rs:pg_versions}}
```

The version marker pins the capability set, not the builder: the same statement composes for any pinned release, and only the clauses that release does not support stay unreachable:

```rust,no_run
{{#rustdoc_include ../examples/dialects.rs:version}}
```

## Dialect Capability Gates

Features that are not supported across all database engines are gated by compile-time capability traits. Unsupported methods do not exist on incompatible dialect targets, preventing invalid SQL queries at compile time:

| Capability | PostgreSQL | SQLite 3.0+ | SQLite 3.24+ | SQLite 3.35+ |
|---|---|---|---|---|
| `RETURNING` (`returning`) | Supported | Unavailable | Unavailable | Supported |
| `ON CONFLICT` (`on_conflict`) | Supported | Unavailable | Supported | Supported |
| `UPDATE ... FROM` (`from_table`) | Supported | Unavailable | Unavailable | Supported |
| `DELETE ... USING` (`using_table`) | Supported | Unavailable | Unavailable | Unavailable |
| `DISTINCT ON` (`distinct_on`) | Supported | Unavailable | Unavailable | Unavailable |
| `LATERAL JOIN` (`cross_join_lateral`) | Supported | Unavailable | Unavailable | Unavailable |
| Row-Level Locking (`for_update`, `for_share`) | Supported | Unavailable | Unavailable | Unavailable |

Attempting to call an unsupported capability results in a compile-time rejection:

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

## PostgreSQL Extensions

PostgreSQL ships capabilities the embedded engine does not have, and every one of them is an ordinary method on the typed builder.

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

## SQLite Capability Thresholds

SQLite gained its clauses across releases, so the release marker that pins the engine decides which of them exist:

| Clause | Introduced in |
|---|---|
| `ON CONFLICT` | SQLite 3.24 |
| `RETURNING` | SQLite 3.35 |
| `UPDATE ... FROM` | SQLite 3.35 |
| JSON functions (`json_object`, `json_group_array`) | SQLite 3.38 |

An upsert is reachable from the SQLite 3.24 module:

```rust,no_run
{{#rustdoc_include ../examples/dialects.rs:sqlite_upsert}}
```

`RETURNING` needs the next threshold, so it is reachable from the SQLite 3.35 module:

```rust,no_run
{{#rustdoc_include ../examples/dialects.rs:sqlite_returning}}
```

## Embedded SQLite (Synchronous Execution)

SQLite is the embedded engine: the adapter runs over a synchronous connection, and a private in-memory database is what a test suite or a local deployment opens. The fragment below is the synchronous runtime fragment of this chapter, so the connection is opened once and every terminal call of the engine returns its result directly instead of a future.

```rust,no_run
{{#rustdoc_include ../examples/sync_execution.rs:open_in_memory}}
```
