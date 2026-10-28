# `GROUP BY` and `HAVING`

The grouping γ folds rows into groups. `.group_by(...)` is available only on a query without a projection and without a lock, and moves it into a **different type** — `GroupedQuery`:

```rust
# extern crate rurqe;
# use rurqe::core::query::EmptyGroupSet;
# use rurqe::core::{Dialect, HasProjection};
pub struct GroupedQuery<
    D: Dialect,
    Tables = (),
    Output = (),
    GroupByCols = EmptyGroupSet,
    Projected = HasProjection,
> { /* ... */ }
# fn main() {}
```

The meaning of the transition: the rule "a column is either grouped or aggregated" requires knowledge of the grouping, and after `.select(...)` the set of projections is already fixed, so there would be nothing to check it against.

```rust,no_run
# extern crate rurqe;
# use rurqe::postgres::prelude::*;
# table! { User => "users" { UserId => "id": i64, } }
# table! { Order => "orders" { OrderTotal => "total": f64, OrderUserId => "user_id": i64, } }
# fn main() {
{{#rustdoc_include ../../../../examples/aggregates.rs:group_by}}
# }
```

```text
SELECT "users"."id" AS "user_id", COUNT(*)
FROM "users" INNER JOIN "orders" ON ("orders"."user_id" = "users"."id")
GROUP BY "users"."id"
HAVING (COUNT(*) > $1)
```

A single column or a tuple can be grouped: `.group_by((UserId, UserName))`.

## What can be projected

`GroupedSelectTarget<GroupByCols, P, D>` is implemented for exactly four cases:

| Projection | Condition |
|---|---|
| a column | it is part of `GROUP BY` (`ColumnInGroupBy`) |
| an aggregate `Expr<T, Scope, Marker, ..>` | `Marker: AggregateMarker` |
| a constant expression `Expr<T, (), NonAggregate, ..>` | reads no table |
| a window expression | the window columns are grouped, and the operand aggregates rows or is constant |

Projecting an ungrouped column is rejected at the call site, and a `select!` selection receives the same check through `GroupedSelection`.

The `schema` escape hatch `Expr::column::<C>()` does not bypass the rule: an expression reading a column keeps that column's scope. `without_scope()` does not help either — it substitutes the `Unscoped` marker, not an empty scope, so the expression does not become a constant.

## The aggregate functions

| Call | SQL |
|---|---|
| `count_all()` | `COUNT(*)` |
| `count(col)` | `COUNT(col)` |
| `count_distinct(col)` | `COUNT(DISTINCT col)` |
| `sum(col)` / `sum_distinct(col)` | `SUM(col)` / `SUM(DISTINCT col)` |
| `avg(col)` / `avg_distinct(col)` | `AVG(col)` / `AVG(DISTINCT col)` |
| `min(col)` / `max(col)` | `MIN(col)` / `MAX(col)` |

`count_all()` counts rows; `count(col)` counts the rows where the column is present — the distinction the two spellings carry.

An aggregate call is an expression like any other: it compares — `.having(count_all().gt(0i64))` above — and it travels through the arithmetic and the scalar functions of [Expressions](expressions.md). The call-attached `DISTINCT` modifier and `FILTER (WHERE ...)` exist on the call alone; an expression built from the call, such as `sum(OrderTotal) + 1.0`, accepts neither.

## Aggregate folding in scalar functions

`coalesce`, `nullif`, `greatest`, `least`, and `CASE` propagate the aggregate marker through `FunctionOperand` / `CombineAggr`, so `coalesce(sum(orders.total), 0.0)` stays an aggregate and projects from a `GroupedQuery`. A `CASE` branch reading a bare column makes the expression per-row, and the grouping rejects it.

## `HAVING`

`HAVING` filters the already computed groups, so it is written over an aggregate. The example above already uses `.having(count_all().gt(0i64))`; it renders as `HAVING (COUNT(*) > $1)`.

Two kinds of predicate are allowed (`HavingPredicate`):

- an aggregate one (`Marker: AggregateMarker`) — as above;
- a constant within the group (`Expr<bool, (), NonAggregate, ..>`) — for example a `sql!` fragment comparing two aggregates.

A predicate reading a bare column is rejected: the grouping has discarded the per-row value. A separate form exists for comparing a **grouped** column:

```rust,no_run
# extern crate rurqe;
# use rurqe::postgres::prelude::*;
# table! { Order => "orders" { OrderUserId => "user_id": i64, } }
# fn main() -> Result<(), Box<dyn std::error::Error>> {
let filtered = select_from(Order)
    .group_by(OrderUserId)
    .select((OrderUserId, count_all()))
    .having_col(OrderUserId, Expr::column::<OrderUserId>().gt(10i64))
    .build()?;
# let _ = filtered;
# Ok(())
# }
```

`having` is available only on a `GroupedQuery` — a plain `SelectQuery` has no `HAVING` clause.

## `FILTER (WHERE ...)`

The rows an aggregate call evaluates can be restricted without a `CASE WHEN` detour: the clause is written on the call itself and travels with the expression wherever the caller projects it.

```rust,no_run
# extern crate rurqe;
# use rurqe::postgres::prelude::*;
# table! { Order => "orders" { OrderTotal => "total": f64, OrderStatus => "status": String, } }
# fn main() {
{{#rustdoc_include ../../../../examples/aggregates.rs:filtered}}
# }
```

`filter_where` obeys the same belonging rule as the call's `DISTINCT` modifier:

* the method exists on the aggregate call alone — an expression built from the call, such as `sum(OrderTotal) + 1.0`, is an `Aggregate` but no longer the call, so it accepts no clause: there is nothing left for the clause to restrict;
* repeated calls accumulate their predicates with `AND` on the same call instead of replacing each other;
* the predicate is recorded in the expression's tree rather than in the statement, so the aggregate stays a dialect-neutral value, and the rendering is the native `... FILTER (WHERE ...)` form gated by the `SupportsAggregateFilter` capability trait.

## Ordering and paginating groups

| Method | What it orders |
|---|---|
| `order_by` / `order_by_asc` / `order_by_desc` / `order_by_nulls` | a grouped column (checked by `ColumnInGroupBy`) |
| `order_by_expr` / `order_by_expr_asc` / `order_by_expr_desc` / `order_by_expr_*_nulls_*` | an aggregate (`Marker: AggregateMarker`) |
| `limit` / `offset` | the number of groups |

```rust,no_run
# extern crate rurqe;
# use rurqe::postgres::prelude::*;
# table! { Order => "orders" { OrderUserId => "user_id": i64, } }
# fn main() -> Result<(), Box<dyn std::error::Error>> {
let top = select_from(Order)
    .group_by(OrderUserId)
    .select((OrderUserId, count_all()))
    .order_by_expr_desc(count_all())
    .limit(10)
    .build()?;
# let _ = top;
# Ok(())
# }
```

## Finishing a grouped query

- `.build()` — only after `.select(...)` (`ProjectionPresent`);
- `.as_cte::<T>()` and `.as_table::<T>()` — nest the aggregated query as a CTE or a derived table;
- a further `.group_by(...)` is possible only before the projection.

A grouped query is not locked: `GroupedQuery` provides no `for_update`/`for_share`, because PostgreSQL does not lock the rows of a statement that yields one row per group.

## What `.build()` adds

The residual cases that cannot be expressed as a positive assertion in types: `BuildError::UngroupedColumn`, `UngroupedHavingColumn`, `UngroupedColumnWithAggregate`.

> **Compiler Guard in Action.** `OrderTotal` is not part of `GROUP BY` and is not aggregated — `select((OrderUserId, OrderTotal))` does not compile, with a message saying the column is "neither grouped nor an aggregate".

## Related chapters

- [Expressions](expressions.md) — arithmetic, `CASE`, and the scalar functions an aggregate folds through.
- [Window functions](window.md) — a window over groups.
- [Projections and `SELECT`](select.md)
- [NF² and MULTISET](../theory/nf2_multiset.md) — an aggregated subquery as a derived table.
