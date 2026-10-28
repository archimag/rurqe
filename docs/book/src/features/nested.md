# Nested results: `nest_many` and `nest_one`

A one-to-many relation is where the ergonomics of a query builder are decided. The naive options cost either N+1 statements (one per parent) or a `JOIN` with a Cartesian product that the client folds into lists by hand.

RURQE reads the relation in one statement: the engine aggregates the child rows of every parent into a JSON document, and the client decodes that document into the row type of the child projection.

## One query, typed collections

```rust,no_run
# extern crate rurqe;
use rurqe::postgres::prelude::*;
# table! { User => "users" { UserId => "id": i64, UserName => "name": String, UserActive => "active": bool, } }
# table! { Order => "orders" { OrderId => "id": i64, OrderUserId => "user_id": i64, OrderTotal => "total": f64, } }
# fn main() -> Result<(), Box<dyn std::error::Error>> {
# let mut conn: PgConnection = todo!();

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
    .where_(UserActive.eq(true))
    .fetch_all(&mut conn)?;

for user in users {
    println!("{} — {} orders", user.user_name, user.orders.len());
    for order in user.orders {
        println!("  #{} for {}", order.order_id, order.total);
    }
}
# Ok(())
# }
```

`user.orders` is a `Vec` of a record with `order_id` and `total`; nothing is decoded by index, and no second statement was sent.

## `nest_one`: at most one row

The same call for a one-to-one relation stops at the first row and reads as `Option<...>`:

```rust,no_run
# extern crate rurqe;
use rurqe::postgres::prelude::*;
# table! { User => "users" { UserId => "id": i64, } }
# table! { Order => "orders" { OrderId => "id": i64, OrderUserId => "user_id": i64, OrderTotal => "total": f64, } }
# fn main() -> Result<(), Box<dyn std::error::Error>> {
# let mut conn: PgConnection = todo!();

let users = select_from(User)
    .select(select!(
        user_id: UserId,
        latest: nest_one(
            select_from(Order)
                .select(select!(total: OrderTotal))
                .where_(OrderUserId.eq(UserId.without_scope()))
                .order_by_desc(OrderId)
        ),
    ))
    .fetch_all(&mut conn)?;

for user in users {
    match user.latest {
        Some(latest) => println!("#{}: {}", user.user_id, latest.total),
        None => println!("#{}: no orders", user.user_id),
    }
}
# Ok(())
# }
```

An absent child is `None`, not an empty collection: "no row" and "a row without values" stay distinguishable.

## Correlating the child with the parent

The nested statement is a query of its own, so its scope holds only its own `FROM`. A column of the outer query enters the inner predicate through `.without_scope()`, which marks the expression as resolved by the enclosing statement:

```rust,no_run
# extern crate rurqe;
# use rurqe::postgres::prelude::*;
# table! { User => "users" { UserId => "id": i64, } }
# table! { Order => "orders" { OrderUserId => "user_id": i64, OrderTotal => "total": f64, } }
# fn main() {
select_from(Order)
    .select(select!(total: OrderTotal))
    .where_(OrderUserId.eq(UserId.without_scope()));
# }
```

Without it the child query would reference `"users"`, a relation it never declares, and the call is rejected at the call site.

## The outer projection is `select!`

`nest_many(...)` and `nest_one(...)` are computed projections, so the outer list is a `select!`. `dto!` accepts column markers only, and its message points at `select!` for computed projections.

## The keys are the field names

The nested `select!` names the JSON keys of every child row: `select!(order_id: OrderId, total: OrderTotal)` produces documents of the shape `{"order_id": …, "total": …}`. The declaration and the document cannot drift apart, because both come from the same list.

## What the child query controls

The filter, the ordering, the row limit, and any grouping of the nested statement decide the contents of the collection — not the client. `nest_one` adds a limit of one row on top, so the engine never sends the children of one parent to be trimmed afterwards.

## Which engines

| Engine | Nested collection |
|---|---|
| PostgreSQL (`rurqe::postgres::prelude`) | supported |
| SQLite 3.38+ (`rurqe::sqlite::sqlite3_38::prelude`) | supported, through the JSON functions |
| SQLite below 3.38 | not available — the projection does not compile |

The gate is the dialect's JSON aggregation capability. SQLite shipped the JSON functions as an opt-in extension until 3.38, so a default build of an older release would reject the statement at runtime; the projection is refused at compile time instead. See [Engines, capability gates, and decoding](multiset_engines.md).

## Compiler Guard in Action

- A nested statement without a projection (`nest_many(select_from(Order))`) does not compile: there is no row type to collect.
- A dialect without JSON aggregation does not accept the projection at all, so an unsupported engine fails at the type level instead of inside the driver.
- The element type has to declare its keys, which is what the `select!` record does.

## Related chapters

- [MULTISET: how nesting becomes SQL](multiset.md)
- [Engines, capability gates, and decoding](multiset_engines.md)
- [NF² and MULTISET: nested relations](../theory/nf2_multiset.md)
