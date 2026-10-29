# Quickstart in five minutes

A working query in a few minutes: a typed `INSERT ... RETURNING`, a named projection, a `LEFT JOIN` whose nullability the compiler infers, and one statement that returns a nested collection. Building a statement never opens a connection, so everything below compiles without a database — and every fragment is compiled by the workspace build, so a chapter cannot drift from the API it documents.

## 1. Dependency

```toml
[dependencies]
rurqe = { version = "0.1", features = ["sqlite", "macros"] }
# for PostgreSQL: features = ["postgres", "macros"]
```

| Feature | What it adds |
|---|---|
| `sqlite` | `Sqlite<V>`, `SqliteConnection`, the versions `Sqlite3_0` … `Sqlite3_38` |
| `postgres` | `PgConnection` — the synchronous `postgres` driver |
| `tokio-postgres`, `deadpool-postgres` | the asynchronous connection and pool |
| `macros` | `select!`, `cols!`, `dto!`, `sql!`, `raw_sql!`, `#[derive(SqlType)]` |
| `migrate` | the embedded migration runner |
| `uuid`, `chrono`, `serde_json`, `decimal` | ecosystem value types |

## 2. Schema

```rust
# extern crate rurqe;
# use rurqe::prelude::*;
table! {
    /// The `users` table.
    User => "users" {
        UserId => "id": i64,
        UserName => "name": String,
        UserEmail => "email": String,
    }
}
# fn main() {}
```

A real project rarely writes this by hand: `cargo db-codegen` generates the same zero-sized markers from the live catalog.

## 3. A typed read

A filtered projection, executed on an asynchronous pool:

```rust,no_run
# extern crate rurqe;
# use rurqe::postgres::prelude::*;
# table! { User => "users" { UserId => "id": i64, UserName => "name": String, UserEmail => "email": String, } }
# async fn example() -> Result<()> {
#     let pool: PgAsyncPool = todo!();
{{#rustdoc_include ../../../../examples/quickstart.rs:query}}
#     Ok(())
# }
# fn main() {}
```

`users` is a vector of the produced row: one field per projection, named after the marker (`UserId` → `user_id`), which is why no tuple index appears anywhere. Assembling the statement opened no connection — `.fetch_all(&pool).await?` is the terminal call, and it is the only place the runtime appears, since the identical chain ends with `.fetch_all(&mut conn)?` against a synchronous connection. `build()` stays available on its own, and it is the one step that can refuse a syntactically sound but semantically invalid statement, so the refusal arrives before any network I/O.

## 4. A write that returns the row

```rust,no_run
# extern crate rurqe;
# use rurqe::postgres::prelude::*;
# table! { User => "users" { UserId => "id": i64, UserName => "name": String, UserEmail => "email": String, } }
# async fn example() -> Result<()> {
#     let pool: PgAsyncPool = todo!();
{{#rustdoc_include ../../../../examples/crud.rs:insert}}
#     Ok(())
# }
# fn main() {}
```

`values` checks every value against the column's Rust type at the call site, so `UserId::set("1")` does not compile. `returning` is gated by the engine: on a version that cannot return a row, the call is not a method at all.

## 5. Mutations

```rust,no_run
# extern crate rurqe;
# use rurqe::postgres::prelude::*;
# table! { User => "users" { UserId => "id": i64, UserName => "name": String, } }
# async fn example() -> Result<()> {
#     let pool: PgAsyncPool = todo!();
{{#rustdoc_include ../../../../examples/crud.rs:update}}
#     Ok(())
# }
# fn main() {}
```

```rust,no_run
# extern crate rurqe;
# use rurqe::postgres::prelude::*;
# table! { User => "users" { UserId => "id": i64, UserName => "name": String, } }
# async fn example() -> Result<()> {
#     let pool: PgAsyncPool = todo!();
{{#rustdoc_include ../../../../examples/crud.rs:delete}}
#     Ok(())
# }
# fn main() {}
```

`update(...)` and `delete_from(...)` start unfiltered: the statement is not buildable until `.where_(...)` or an explicit `.allow_all()` is written, so an accidental full-table mutation is impossible by construction. An upsert appends an `ON CONFLICT` clause of its own:

```rust,no_run
# extern crate rurqe;
# use rurqe::postgres::prelude::*;
# table! { User => "users" { UserId => "id": i64, UserName => "name": String, } }
# async fn example() -> Result<()> {
#     let pool: PgAsyncPool = todo!();
{{#rustdoc_include ../../../../examples/crud.rs:upsert}}
#     Ok(())
# }
# fn main() {}
```

## 6. Joins lift nullability

```rust,no_run
# extern crate rurqe;
# use rurqe::postgres::prelude::*;
# table! { User => "users" { UserId => "id": i64, UserName => "name": String, } }
# table! { Order => "orders" { OrderId => "id": i64, OrderTotal => "total": f64, OrderUserId => "user_id": i64, } }
# async fn example() -> Result<()> {
#     let pool: PgAsyncPool = todo!();
{{#rustdoc_include ../../../../examples/joins.rs:outer}}
#     Ok(())
# }
# fn main() {}
```

`order_total` is `Option<f64>` in the projected row because the join may leave the right relation unmatched — and reading it as `f64` does not compile. Nothing in the query mentions a nullable type: the projection is written once and the type follows from the join.

## 7. One query instead of N+1

A one-to-many relation comes back as a typed collection in a single round trip, without a Cartesian product folded on the client:

```rust,no_run
# extern crate rurqe;
# use rurqe::postgres::prelude::*;
# table! { User => "users" { UserId => "id": i64, UserName => "name": String, } }
# table! { Order => "orders" { OrderId => "id": i64, OrderTotal => "total": f64, OrderUserId => "user_id": i64, } }
# async fn example() -> Result<()> {
#     let pool: PgAsyncPool = todo!();
{{#rustdoc_include ../../../../examples/nested.rs:nested}}
#     Ok(())
# }
# fn main() {}
```

## 8. No turbofish anywhere

One import pins the dialect, so entry points take a table value and no type parameter is ever spelled out:

```rust,no_run
# extern crate rurqe;
# use rurqe::postgres::prelude::*;
# table! { User => "users" { UserId => "id": i64, } }
# fn main() {
{{#rustdoc_include ../../../../examples/dialects.rs:postgres}}
# }
```

The same query on SQLite differs only in the prelude and the placeholder style (`?` instead of `$1`):

```rust,no_run
# extern crate rurqe;
# use rurqe::sqlite::{sqlite3_0, sqlite3_35};
# table! { User => "users" { UserId => "id": i64, } }
# fn main() {
{{#rustdoc_include ../../../../examples/dialects.rs:sqlite}}
# }
```

Version modules (`pg14`, `pg15`, `pg16`, `sqlite3_0` … `sqlite3_38`) also fix the capability set: `RETURNING` needs SQLite 3.35, `ON CONFLICT` needs 3.24, and a clause the pinned version lacks is not a method at all.

## 9. Building and executing are separate steps

Assembling a statement opens no connection; the terminal call is what executes it, and that call sits on the query itself. The family covers every shape a query can have — `fetch_one`, `fetch_optional`, `fetch_all`, `fetch_collect`, `fetch_iter`, and `execute` for a mutation, which reads no rows back — and the runtime is chosen there and nowhere else: `.fetch_all(&pool).await?` on an asynchronous pool, `.fetch_all(&mut conn)?` on a synchronous connection. `build()` remains the non-executing form of the same step, and it is the one place that can refuse a statement before any I/O.

## 10. Errors are types

A statement rejected before it reaches the engine is a typed `BuildError`; a driver failure is an `ExecutionError`, which separates a missing row (`is_not_found`), extra rows (`is_multiple_rows`), a decoding failure (`is_row_decode`), and a driver failure (`as_backend`, `into_backend`). Neither path panics.

## 11. What next

- [Projections and `SELECT`](../core/select.md) — `select!`, `cols!`, `dto!`, and the row type.
- [Predicates and `WHERE`](../core/filtering.md) and [ordering](../core/ordering.md).
- [Joins](../core/joins.md) and [nullability](../theory/nullability.md) — why the type changes.
- [Two-tier SQL correctness](two_tier.md) — which gate catches which mistake.
