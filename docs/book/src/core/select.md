# Projections and `SELECT`

A projection decides which attributes travel to the client and in which order. In RURQE it is written once at the call site, and it fixes the row type of the query: decoding, `RETURNING`, a nested collection, and a derived table all read that same shape.

## `select!`: named fields, no tuple index

```rust
use rurqe::postgres::prelude::*;

let active = select!(UserId, UserName as contact_email)
    .from(User)
    .where_(UserActive.eq(true))
    .order_by_asc(UserId)
    .fetch_all(&mut conn)?;

for row in active {
    println!("#{} <{}>", row.user_id, row.contact_email);
}
```

The field names come from the markers: `UserId` becomes `user_id`, and `UserName as contact_email` becomes `contact_email`. A marker named after a Rust keyword is written with a raw identifier (`Type` is `r#type`) and still carries the bare keyword as its output column.

`cols!` is the same macro under a second name — the spelling that reads better when the list is handed to `.select(...)`:

```rust
let rows = select_from(User)
    .select(cols!(UserId, UserName as contact_email))
    .fetch_all(&mut conn)?;
```

Both forms produce the same record and the same SQL, and every element is rendered under its field name. That naming is what keeps the columns reachable when the query becomes a derived table or a CTE.

A positional tuple projection still exists for low-level code; this book does not use it, because a named projection survives a change in the column order.

## Computed projections

Anything that renders as a value can be a field — write the field name yourself:

```rust
let rows = select_from(Order)
    .select(select!(
        order_id: OrderId,
        total: OrderTotal,
        billed: coalesce(OrderTotal, 0.0f64),
    ))
    .fetch_all(&mut conn)?;
```

`billed` has the type of the expression (`f64`), not the type of a stored column.

## Two fields, one name

Duplicate field names are refused while the macro expands, and the message carries the remedy:

```text
projection field `id` is produced by more than one projection; rename one of them with `as <alias>` or `alias: <expr>`
```

## `dto!`: a named row shape

`select!` is anonymous — exactly right inside a query, useless in a signature. `dto!` declares the shape once and reads the field types from the schema instead of repeating them:

```rust
dto! {
    /// A user row crossing a repository boundary.
    pub struct UserSummary {
        UserId,
        UserName as contact_email,
        UserActive,
    }
}
```

The declaration implements `FromRow`, so a row decodes by field name:

```rust
let row = UserSummary::from_row(&[
    SqlVal::I64(7),
    SqlVal::Text("root".to_owned()),
    SqlVal::Bool(true),
])
.expect("row decodes");

assert_eq!(row.user_id, 7);
assert_eq!(row.contact_email, "root");
```

It also implements the nested-row contracts, so `Vec<UserSummary>` decodes straight out of a collection document (see [Nested results](../features/nested.md)).

A DTO is a row shape, not a projection: its fields must be column markers, and a computed field is refused during expansion with a message that points at `select!`. The statement projects with `select!`; the DTO names what the row is.

## Nullability comes from the join

Write the left join and keep the same projection:

```rust
let report = select_from(User)
    .left_join(Order, OrderUserId.eq(UserId))
    .select(select!(user_id: UserId, order_total: OrderTotal))
    .fetch_all(&mut conn)?;

for row in report {
    match row.order_total {
        Some(total) => println!("#{}: {total}", row.user_id),
        None => println!("#{}: no orders", row.user_id),
    }
}
```

```text
SELECT "users"."id" AS "user_id", "orders"."total" AS "order_total"
FROM "users" LEFT JOIN "orders" ON ("orders"."user_id" = "users"."id")
```

`row.order_total` is `Option<f64>`, and nothing in the projection had to mention it — the type follows from the join, so reading the field as `f64` does not compile. The same lifting applies to `FULL OUTER JOIN` (both sides) and to a left-joined derived table; see [Nullability](../theory/nullability.md).

## Grouped queries project under the same rule

```rust
let rows = select_from(Order)
    .group_by(OrderUserId)
    .select(select!(
        user_id: OrderUserId,
        orders: count_all(),
        spend: sum(OrderTotal),
    ))
    .fetch_all(&mut conn)?;
```

After `.group_by(...)` a field is accepted only when it is a grouped column, an aggregate, a constant, or a window over those. A stored column that is neither is refused at the call site, with a message saying the column is neither grouped nor an aggregate. Aggregates inside scalar functions keep that property: `coalesce(sum(OrderTotal), 0.0f64)` is still an aggregate.

## What `RETURNING` accepts

`RETURNING` takes the same named lists:

```rust
let renamed = update(User)
    .set(UserName::set("root"))
    .where_(UserId.eq(1i64))
    .returning(cols!(UserName));
```

The clause writes back into the mutated row, so it accepts only a projection of that row: a column of another table names a relation the clause cannot address, and a computed value is a value the clause cannot write back. Both are refused at the call site, before any SQL is rendered.

## The nullable shape, explicitly

Sometimes the nullable form is needed without an outer join — to align a projection with an already computed result. `query.nullable::<OrderTotal>()` is the same column with `Option<f64>` as its value type; a second layer never appears, because `Option<Option<T>>` collapses into `Option<T>`.

## Invariants worth knowing

- A query without a projection does not render: `.build()` and the fetch helpers are not available on it.
- A projection is recorded once. After `UNION` / `INTERSECT` / `EXCEPT` the output columns are the ones the first list named, so `.select(...)` is unreachable on the combined statement.
- A hand-written selection that records nothing is rejected while building, instead of rendering `SELECT  FROM "users"`.

## Related chapters

- [Predicates and `WHERE`](filtering.md)
- [Ordering and pagination](ordering.md)
- [`GROUP BY` and `HAVING`](group_by.md)
- [Aliases](aliasing.md)
