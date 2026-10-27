# Quickstart in 5 minutes

The goal of this section is a working query plus an understanding of what exactly the compiler checks. Every fragment below corresponds to real code: the scenario comes from the integration test `crates/rurqe/tests/sqlite_e2e.rs`.

## 1. Dependency and features

```toml
[dependencies]
rurqe = { version = "0.1", features = ["sqlite"] }
# or for PostgreSQL:
# rurqe = { version = "0.1", features = ["postgres"] }
```

Adapters and their features:

| Feature | What it adds |
|---|---|
| `postgres` | `PgConnection` — the synchronous `postgres` driver |
| `tokio-postgres` | `PgAsyncConnection` — the asynchronous driver |
| `deadpool-postgres` | `PgAsyncPool` — the asynchronous pool |
| `r2d2` | `PgPool` / `SqlitePool` — the synchronous pools |
| `sqlite` | `Sqlite<V>`, `SqliteConnection`, the versions `Sqlite3_0` ... `Sqlite3_38` |
| `macros` | `select!`, `cols!`, `dto!`, `sql!`, `raw_sql!`, `#[derive(SqlType)]` |
| `migrate` | the embedded migration runner `EmbeddedMigrations` |

Every PostgreSQL adapter feature compiles the dialect surface, so `Postgres`, `Pg14`, `Pg15`, and `Pg16` are available even in a build without the synchronous driver.

## 2. Schema

```rust
use rurqe::prelude::*;

// Manual declaration: one ZST per table and one ZST per column.
table! {
    /// The `users` table.
    User => "users" {
        UserId => "id": i64,
        UserName => "name": String,
        UserActive => "active": bool,
        UserScore => "score": i32,
    }
}
```

A real project never writes this by hand: `cargo db-codegen` generates it from the live database catalog.

## 3. Connection

```rust
use rurqe::sqlite::rusqlite::Connection;

let connection = Connection::open_in_memory()?;
connection.execute_batch(
    "CREATE TABLE users (\
         id INTEGER NOT NULL PRIMARY KEY,\
         name TEXT NOT NULL,\
         active BOOLEAN NOT NULL,\
         score INTEGER NOT NULL);",
)?;
let mut db = SqliteConnection::new(connection);
```

## 4. Writing and reading

```rust
use rurqe::sqlite::sqlite3_35::prelude::*;

// INSERT ... RETURNING: returns the projected row.
let insert = insert_into(User)
    .values((
        UserId::set(1i64),
        UserName::set("root"),
        UserActive::set(true),
        UserScore::set(42i32),
    ))
    .returning((UserId, UserName, UserActive, UserScore));

let row: (i64, String, bool, i32) = db.fetch_one(&insert)?;
assert_eq!(row, (1i64, "root".to_owned(), true, 42i32));

// SELECT with a typed predicate and ordering.
let query = select_from(User)
    .select((UserId, UserName, UserScore))
    .where_(UserActive.eq(true))
    .order_by_asc(UserId);

let rows: Vec<(i64, String, i32)> = db.fetch_all(&query)?;
```

`values` checks the type of every value against `Column::RustType` at the call site, so `UserId::set("1")` does not compile.

## 5. Mutations

```rust
let update = update(User)
    .set(UserName::set("bob"))
    .where_(UserId.eq(1i64))
    .returning((UserName, UserScore));
let renamed: (String, i32) = db.fetch_one(&update)?;

let delete = delete_from(User).where_(UserId.eq(1i64));
let affected: usize = db.execute(&delete)?;
assert_eq!(affected, 1);
```

`update(...)` and `delete_from(...)` start in the unfiltered state: `build()` stays unavailable until `.where_(...)` or an explicit `.allow_all()` is called. An accidental full-table mutation is impossible by construction.

## 6. No turbofish: dialect preludes

One import pins the dialect, so table markers are passed by value and no type parameter appears at the call site:

```rust
use rurqe::sqlite::sqlite3_35::prelude::*;

let query = select_from(User).select(cols!(UserId, UserName));
let rows: Vec<(i64, String)> = db.fetch_all(&query)?;
```

For PostgreSQL the same shape is pinned to a version marker:

```rust
use rurqe::postgres::pg16::prelude::*;

let built = select_from(Users).select(cols!(UserId)).build()?;
```

The generated module defines `select_from`, `insert_into`, `update`, and `delete_from`, the `Error` and `Result<T, E = Error>` aliases, the `SelectFrom` extension trait behind `.from(table)`, and a nested `prelude` that re-exports the shared prelude with the pinned entry points taking precedence.

## 7. Errors are types

```rust
let query = select_from(User)
    .select((UserName,))
    .where_(UserId.eq(1i64));

match db.fetch_one(&query) {
    Ok((name,)) => println!("{name}"),
    Err(error) if error.is_not_found() => println!("no such row"),
    Err(error) => return Err(error.into()),
}
```

`ExecutionError<E>` distinguishes query construction (`is_build`), a missing row (`is_not_found`), extra rows (`is_multiple_rows`), a decoding failure (`is_row_decode`), and a driver failure (`as_backend`, `into_backend`).

## 8. What next

- Why `LEFT JOIN` changes a column's type — [Nullability](../theory/nullability.md).
- Where the boundary between compiler checks and `.build()` runs — [Two-Tier SQL Correctness](two_tier.md).
- How to get rid of a hand-written `table!` — the code generation chapter (a separate section of the book).
