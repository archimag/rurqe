# Quickstart in five minutes

A working query in three minutes: a typed `INSERT ... RETURNING`, a named projection through `select!`, and a `LEFT JOIN` whose nullability the compiler infers on its own. Every fragment below is included from the `examples/` crate, so `cargo check --workspace --all-targets` compiles it and `mdbook test` runs it; no code is written inline.

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

A real project never writes this by hand: `cargo db-codegen` generates the same zero-sized markers from the live catalog.

## 3. A typed read

`examples/quickstart.rs` builds a filtered projection and prints the SQL it produced:

```rust,no_run
# extern crate rurqe;
# use rurqe::postgres::prelude::*;
# table! { User => "users" { UserId => "id": i64, UserName => "name": String, UserEmail => "email": String, } }
# fn main() {
{{#rustdoc_include ../../../../examples/quickstart.rs:query}}
# }
```

`built` is a `BuiltQuery`: `sql` holds the statement and `params` its parameters. The projection is written once with `cols!`; the field names come from the markers (`UserId` → `user_id`), which is why no tuple index appears anywhere.

## 4. A write that returns the row

```rust,no_run
# extern crate rurqe;
# use rurqe::postgres::prelude::*;
# table! { User => "users" { UserId => "id": i64, UserName => "name": String, UserEmail => "email": String, } }
# fn main() {
{{#rustdoc_include ../../../../examples/crud.rs:insert}}
# }
```

`values` checks every value against the column's Rust type at the call site, so `UserId::set("1")` does not compile. `returning` is gated by `SupportsReturning`.

## 5. Mutations

```rust,no_run
# extern crate rurqe;
# use rurqe::postgres::prelude::*;
# table! { User => "users" { UserId => "id": i64, UserName => "name": String, } }
# fn main() {
{{#rustdoc_include ../../../../examples/crud.rs:update}}
# }
```

```rust,no_run
# extern crate rurqe;
# use rurqe::postgres::prelude::*;
# table! { User => "users" { UserId => "id": i64, UserName => "name": String, } }
# fn main() {
{{#rustdoc_include ../../../../examples/crud.rs:delete}}
# }
```

`update(...)` and `delete_from(...)` start unfiltered: the statement is not buildable until `.where_(...)` or an explicit `.allow_all()` is called, so an accidental full-table mutation is impossible by construction. An upsert appends an `ON CONFLICT` clause of its own:

```rust,no_run
# extern crate rurqe;
# use rurqe::postgres::prelude::*;
# table! { User => "users" { UserId => "id": i64, UserName => "name": String, } }
# fn main() {
{{#rustdoc_include ../../../../examples/crud.rs:upsert}}
# }
```

## 6. Joins lift nullability

```rust,no_run
# extern crate rurqe;
# use rurqe::postgres::prelude::*;
# table! { User => "users" { UserId => "id": i64, UserName => "name": String, } }
# table! { Order => "orders" { OrderId => "id": i64, OrderTotal => "total": f64, OrderUserId => "user_id": i64, } }
# fn main() {
{{#rustdoc_include ../../../../examples/joins.rs:outer}}
# }
```

`order_total` is `Option<f64>` in the projected row because the join may leave the right relation unmatched — and reading it as `f64` does not compile. Nothing in the query mentions a nullable type: the projection is written once and the type follows from the join.

## 7. One query instead of N+1

A one-to-many relation comes back as a typed collection in a single round trip, without a Cartesian product folded on the client:

```rust,no_run
# extern crate rurqe;
# use rurqe::postgres::prelude::*;
# table! { User => "users" { UserId => "id": i64, UserName => "name": String, } }
# table! { Order => "orders" { OrderId => "id": i64, OrderTotal => "total": f64, OrderUserId => "user_id": i64, } }
# fn main() {
{{#rustdoc_include ../../../../examples/nested.rs:nested}}
# }
```

Details in [Nested results](../features/nested.md).

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

## 9. Errors are types

A statement rejected before it reaches the engine is a typed `BuildError`; a driver failure is an `ExecutionError`, which separates a missing row (`is_not_found`), extra rows (`is_multiple_rows`), a decoding failure (`is_row_decode`), and a driver failure (`as_backend`, `into_backend`). Neither path panics.

## 10. What next

- [Projections and `SELECT`](../core/select.md) — `select!`, `cols!`, `dto!`, and the row type.
- [Predicates and `WHERE`](../core/filtering.md) and [ordering](../core/ordering.md).
- [Joins](../core/joins.md) and [nullability](../theory/nullability.md) — why the type changes.
- [Nested results](../features/nested.md) — `nest_many` and `nest_one`.
- [Two-tier SQL correctness](two_tier.md) — which gate catches which mistake.
