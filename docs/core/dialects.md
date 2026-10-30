# Dialects & Capabilities

RURQE supports multiple database engines through zero-cost dialect types, version thresholds, and compile-time capability gates. Dialects customize identifier quoting, parameter placeholder formatting, and available SQL clauses without adding runtime checks.

## Pinned Dialect Preludes

Application code binds a dialect by importing its prelude. The prelude supplies builder entry points (`select_from`, `insert_into`, `update`, `delete_from`) and macro helpers pre-configured for the target engine, eliminating explicit generic parameters:

```rust,no_run
{{#rustdoc_include ../examples/dialects.rs:postgres}}
```

The PostgreSQL dialect quotes identifiers with double quotes (`"users"`) and formats numbered parameter placeholders (`$1, $2, ...`).

The SQLite dialect generates double-quoted identifiers and positional parameter placeholders (`?`):

```rust,no_run
{{#rustdoc_include ../examples/dialects.rs:sqlite}}
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

## Version Thresholds

Dialects accept engine version markers to target specific database releases. Version markers conditionally enable capabilities based on database release thresholds while preserving the identical query builder API:

```rust,no_run
{{#rustdoc_include ../examples/dialects.rs:version}}
```

When targeting older database versions, methods for newer SQL features are unavailable at compile time, guaranteeing compatibility with legacy deployments.
