# LATERAL joins

A `LATERAL` subquery may reference columns of the tables to its left in the
`FROM` clause, which lets a join express per-row correlated subqueries —
"the latest order of every user" — directly in the join. The clause is
gated by `SupportsLateralJoin`, which the SQLite dialect does not implement,
so the builders do not exist there.

Turn a projected `SelectQuery` into a lateral relation with `as_lateral()`,
then join it:

```rust
let latest_orders = select_from(orders)
    .select((orders.id, orders.total))
    .where_(orders.user_id.eq(users.id))   // correlated with the left table
    .order_by_desc(orders.created_at)
    .limit(1)
    .as_lateral::<LatestOrder>();

let report = select_from(users)
    .cross_join_lateral(latest_orders)
    .select((users.id, orders.total));
```

The correlated reference — `orders.user_id` against the outer `users.id` —
renders inside the subquery with the outer column bound as an ordinary
parameter of the statement; no client round-trip, no duplicated query.

## Left lateral joins

`.left_join_lateral(…)` keeps users without a matching subquery row, so the
lateral relation is nullable: its projected columns read as `Option<T>` —
the outer-join nullability contract, applied to the derived relation exactly
as it applies to a plain table (see
[nullability](../../theory/nullability.md)).
