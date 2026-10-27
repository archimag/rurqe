# Quickstart in five minutes

A working query in three minutes: an in-memory SQLite database, a typed `INSERT ... RETURNING`, a named projection through `select!`, and a `LEFT JOIN` whose nullability the compiler infers on its own. The CRUD half of the scenario mirrors the integration test `crates/rurqe/tests/sqlite_e2e.rs`.

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
use rurqe::prelude::*;

table! {
    /// The `users` table.
    User => "users" {
        UserId => "id": i64,
        UserName => "name": String,
        UserActive => "active": bool,
        UserScore => "score": i32,
    }
}

table! {
    /// The `orders` table.
    Order => "orders" {
        OrderId => "id": i64,
        OrderUserId => "user_id": i64,
        OrderTotal => "total": f64,
    }
}
```

A real project never writes this by hand: `cargo db-codegen` generates the same zero-sized markers from the live catalog.

## 3. Connection

```rust
use rurqe::sqlite::rusqlite::Connection;

let connection = Connection::open_in_memory()?;
connection.execute_batch(
    "CREATE TABLE users (\
         id INTEGER NOT NULL PRIMARY KEY,\
         name TEXT NOT NULL,\
         active BOOLEAN NOT NULL,\
         score INTEGER NOT NULL);\
     CREATE TABLE orders (\
         id INTEGER NOT NULL PRIMARY KEY,\
         user_id INTEGER NOT NULL,\
         total REAL NOT NULL);",
)?;
let mut db = SqliteConnection::new(connection);
```

## 4. Write and read with `select!`

```rust
use rurqe::sqlite::sqlite3_35::prelude::*;

let inserted = insert_into(User)
    .values((
        UserId::set(1i64),
        UserName::set("root"),
        UserActive::set(true),
        UserScore::set(42i32),
    ))
    .returning(cols!(UserId, UserName as contact_email));

let row = db.fetch_one(&inserted)?;
println!("#{} {}", row.user_id, row.contact_email);

let active = select!(UserId, UserName as contact_email, UserScore)
    .from(User)
    .where_(UserActive.eq(true))
    .order_by_asc(UserId);

for row in db.fetch_all(&active)? {
    println!("#{} {} — {}", row.user_id, row.contact_email, row.user_score);
}
```

`values` checks every value against the column's Rust type at the call site, so `UserId::set("1")` does not compile. The field names of the row come from the markers (`UserId` → `user_id`, `UserName as contact_email` → `contact_email`), which is why no tuple index appears anywhere.

## 5. Mutations

```rust
let renamed = update(User)
    .set(UserName::set("bob"))
    .where_(UserId.eq(1i64))
    .returning(cols!(UserName, UserScore));
let row = db.fetch_one(&renamed)?;

let removed = db.execute(&delete_from(User).where_(UserId.eq(1i64)))?;
assert_eq!(removed, 1);
```

`update(...)` and `delete_from(...)` start unfiltered: the statement is not buildable until `.where_(...)` or an explicit `.allow_all()` is called, so an accidental full-table mutation is impossible by construction.

## 6. Joins lift nullability

```rust
let report = select_from(User)
    .left_join(Order, OrderUserId.eq(UserId))
    .select(select!(
        user_id: UserId,
        user_name: UserName,
        total: OrderTotal,
    ))
    .order_by_asc(UserId);

for row in db.fetch_all(&report)? {
    match row.total {
        Some(total) => println!("{}: {total}", row.user_name),
        None => println!("{}: no orders", row.user_name),
    }
}
```

`row.total` is `Option<f64>` because the join may leave the right relation unmatched — and reading it as `f64` does not compile. Nothing in the query mentions a nullable type: the projection is written once and the type follows from the join.

## 7. One query instead of N+1

A one-to-many relation comes back as a typed collection in a single round trip, without a Cartesian product folded on the client. JSON aggregation is required, so this runs on PostgreSQL (or SQLite 3.38 and later):

```rust
use rurqe::postgres::prelude::*;

let users_with_orders = select_from(User)
    .select(select!(
        user_id: UserId,
        user_name: UserName,
        orders: nest_many(
            select_from(Order)
                .select(select!(order_id: OrderId, total: OrderTotal))
                .where_(OrderUserId.eq(UserId.without_scope()))
                .order_by_asc(OrderTotal)
        ),
    ))
    .fetch_all(&mut conn)?;

for user in users_with_orders {
    println!("{} — {} orders", user.user_name, user.orders.len());
}
```

Details in [Nested results](../features/nested.md).

## 8. No turbofish anywhere

One import pins the dialect, so entry points take a table value and no type parameter is ever spelled out:

```rust
use rurqe::postgres::pg16::prelude::*;   // PostgreSQL 16
use rurqe::sqlite::sqlite3_35::prelude::*; // SQLite 3.35:

let built = select_from(User).select(cols!(UserId)).build()?;
```

Version modules (`pg14`, `pg15`, `pg16`, `sqlite3_0` … `sqlite3_38`) also fix the capability set: `RETURNING` needs SQLite 3.35, `ON CONFLICT` needs 3.24, and a clause the pinned version lacks is not a method at all.

## 9. Errors are types

```rust
let query = select_from(User)
    .select(cols!(UserName))
    .where_(UserId.eq(1i64));

match db.fetch_one(&query) {
    Ok(row) => println!("{}", row.user_name),
    Err(error) if error.is_not_found() => println!("no such row"),
    Err(error) => return Err(error.into()),
}
```

`ExecutionError` separates a rejected statement (`is_build`), a missing row (`is_not_found`), extra rows (`is_multiple_rows`), a decoding failure (`is_row_decode`), and a driver failure (`as_backend`, `into_backend`).

## 10. What next

- [Projections and `SELECT`](../core/select.md) — `select!`, `cols!`, `dto!`, and the row type.
- [Predicates and `WHERE`](../core/filtering.md) and [ordering](../core/ordering.md).
- [Joins](../core/joins.md) and [nullability](../theory/nullability.md) — why the type changes.
- [Nested results](../features/nested.md) — `nest_many` and `nest_one`.
- [Two-tier SQL correctness](two_tier.md) — which gate catches which mistake.
