# SQL Execution: Sync vs Async

RURQE separates query construction from query execution. Query building is purely relational and performs no I/O, allowing identical statements to run against synchronous and asynchronous backends.

## Runtime Orthogonality

Assembling a statement—defining projections, filters, joins, groupings, and mutations—carries no connection or runtime parameter in its signature. The runtime target is chosen exclusively at the terminal call:

* **Asynchronous Execution:** Returns a future consumed with `.await?` (`.fetch_all(&pool).await?`).
* **Synchronous Execution:** Returns a `Result` directly (`.fetch_all(&mut conn)?`).

No preceding method in the query chain changes between synchronous and asynchronous code.

## Terminal Query Methods

Both synchronous and asynchronous backends provide the same terminal execution methods:

| Method | Purpose | Result on 0 rows | Result on 2+ rows |
|---|---|---|---|
| `.fetch_all(...)` | Retrieve all matching rows as `Vec<Row>` | `Ok([])` | `Ok([...])` |
| `.fetch_one(...)` | Retrieve exactly one row | `Err(ExecutionError::NoRows)` | `Err(ExecutionError::MultipleRows)` |
| `.fetch_optional(...)` | Retrieve zero or one row as `Option<Row>` | `Ok(None)` | `Err(ExecutionError::MultipleRows)` |
| `.execute(...)` | Execute a mutation and return affected row count | `Ok(0)` | `Ok(n)` |

## Asynchronous Execution (Tokio)

Asynchronous execution is the primary standard for services and web applications. RURQE integrates natively with `tokio-postgres` and the `deadpool-postgres` connection pool:

```rust,no_run
{{#rustdoc_include ../examples/quickstart.rs:query}}
```

## Synchronous Execution

Synchronous execution is supported for CLI tools, embedded applications, and background workers through `rusqlite` (`SqliteConnection`) and synchronous `postgres` (`PgConnection`):

```rust
use rurqe::sqlite::prelude::*;

table! {
    User => "users" {
        UserId => "id": i64,
    }
}

fn load_user(conn: &mut SqliteConnection) -> Result<Option<i64>, Error> {
    let user = select_from(User)
        .select(cols!(UserId))
        .fetch_optional(conn)?;
    Ok(user.map(|row| row.user_id))
}
```

## Typed Error Handling

Terminal methods return `Result<T, ExecutionError<E>>`, preserving driver-specific errors without type erasure:

* `rurqe::postgres::prelude::*` aliases errors to `ExecutionError<PgError>`.
* `rurqe::sqlite::prelude::*` aliases errors to `ExecutionError<rusqlite::Error>`.

Propagating errors with `?` cleanly bubbles typed failures up to the caller.
