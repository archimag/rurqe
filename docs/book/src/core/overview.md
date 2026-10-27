# Everyday queries: the portable core

This part is the tour of the everyday surface — the half of the DSL that does not depend on a specific engine: projections, filters, ordering, grouping, windows, joins, aliases, subqueries, CTEs, set operations, DML, and nested collections.

Three rules hold across every chapter:

| Rule | What it looks like in your code |
|---|---|
| **No turbofish** | `select_from(User)`, `insert_into(User)`, `update(User)`, `delete_from(User)` — one import pins the dialect, and the table is passed as a value |
| **Named fields** | `select!` / `cols!` synthesize a record with the field names of the projection, so a row is read as `row.user_id`, never `row.0` |
| **Reusable shapes** | `dto!` declares a row shape once, with the field types read from the schema |

## The two tables this part uses

```rust
use rurqe::prelude::*;

table! {
    /// The `users` table.
    User => "users" {
        UserId => "id": i64,
        UserName => "name": String,
        UserActive => "active": bool,
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

In a real project these markers are not written by hand: `cargo db-codegen` reads the live catalog and generates the same zero-sized types, together with the catalog snapshot that CI verifies.

## A query you would actually write

```rust
use rurqe::postgres::prelude::*;

let rows = select!(
    user_id: UserId,
    user_name: UserName,
    total: OrderTotal,
)
.from(User)
.inner_join_table(Order, OrderUserId.eq(UserId))
.where_(UserActive.eq(true))
.order_by_asc(UserId)
.limit(20)
.fetch_all(&mut conn)?;

for row in rows {
    println!("#{} {} — {}", row.user_id, row.user_name, row.total);
}
```

No column type is repeated, no dialect is named, no tuple is indexed. `total` arrives as `f64`; replace `inner_join_table` with `left_join_table` and the same line yields `Option<f64>`, because the join may leave the order unmatched.

## The same query, assembled at runtime

```rust
let rows = select_from(User)
    .select(select!(user_id: UserId, user_name: UserName))
    .when(only_active, |query| query.where_(UserActive.eq(true)))
    .if_some(search, |query, term| query.where_(UserName.ilike(format!("%{term}%"))))
    .order_by_asc(UserId)
    .fetch_all(&mut conn)?;
```

An absent branch adds nothing, and the query keeps its type, so nothing downstream has to be re-typed.

## Aggregations keep the same ergonomics

```rust
let spend = select_from(Order)
    .group_by(OrderUserId)
    .select(select!(
        user_id: OrderUserId,
        orders: count_all(),
        spend: coalesce(sum(OrderTotal), 0.0f64),
    ))
    .having(count_all().gt(1i64))
    .order_by_expr_desc(count_all())
    .limit(10)
    .fetch_all(&mut conn)?;
```

`count_all()` and `sum(OrderTotal)` are ordinary values, `coalesce` keeps the aggregate property of its argument, and the grouped query accepts exactly the columns it grouped by plus those aggregates — anything else is refused at the call site instead of by the engine.

## One query instead of N+1

A one-to-many relation needs neither a second statement nor a client-side fold: the engine aggregates the child rows into a JSON document, and the client decodes it into a typed collection.

```rust
let users = select_from(User)
    .select(select!(
        user_id: UserId,
        user_name: UserName,
        orders: nest_many(
            select_from(Order)
                .select(select!(order_id: OrderId, total: OrderTotal))
                .where_(OrderUserId.eq(UserId.without_scope()))
                .order_by_desc(OrderTotal)
        ),
    ))
    .fetch_all(&mut conn)?;

for user in users {
    println!("{} — {} orders", user.user_name, user.orders.len());
}
```

[Nested results](../features/nested.md) covers `nest_many` and `nest_one` in full, and [MULTISET](../features/multiset.md) shows the SQL that comes out of it.

## What the chapters rely on

| Crate | What these chapters use from it |
|---|---|
| `rurqe-core` | the query builders, the expression model, the pre-flight validation |
| `rurqe-macros` | `select!`, `cols!`, `dto!`, `sql!`, `#[derive(SqlType)]` |
| `rurqe` | the entry points, the prelude, and the version-pinned dialect modules |

Engine specifics — capability gates, versions, dialect extensions — are covered in [Dialects](../dialects/overview.md).

## Chapters of this part

1. [Schema: tables and columns](schema.md)
2. [Projections and `SELECT`](select.md)
3. [Predicates and `WHERE`](filtering.md)
4. [Ordering and pagination](ordering.md)
5. [DML: `INSERT`, `UPDATE`, `DELETE`](dml.md)
6. [Nested results: `nest_many` and `nest_one`](../features/nested.md)
7. [MULTISET: how nesting becomes SQL](../features/multiset.md)
8. [Engines, capability gates, and decoding](../features/multiset_engines.md)

Every chapter follows one scheme: the task, the code, the SQL it produces, and the mistake the compiler refuses.
