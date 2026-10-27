# `GROUP BY` and `HAVING`

The grouping γ folds rows into groups. `.group_by(...)` is available only on a query without a projection and without a lock, and moves it into a **different type** — `GroupedQuery`:

```rust
pub struct GroupedQuery<
    D: Dialect,
    Tables = (),
    Output = (),
    GroupByCols = EmptyGroupSet,
    Projected = HasProjection,
> { /* ... */ }
```

The meaning of the transition: the rule "a column is either grouped or aggregated" requires knowledge of the grouping, and after `.select(...)` the set of projections is already fixed, so there would be nothing to check it against.

```rust
use rurqe::prelude::*;

let built = select_from::<Postgres, Order>()
    .group_by(OrderUserId)
    .select((OrderUserId, Expr::sum::<OrderTotal>()))
    .build()
    .expect("valid grouped query");
```

```text
SELECT "user_id", SUM("total") FROM "orders" GROUP BY "user_id"
```

A single column or a tuple can be grouped: `.group_by((UsersId, UsersName))`.

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

## Aggregate folding in scalar functions

`coalesce`, `nullif`, `greatest`, `least`, and `CASE` propagate the aggregate marker through `FunctionOperand` / `CombineAggr`, so `coalesce(sum(orders.total), 0.0)` stays an aggregate and projects from a `GroupedQuery`. A `CASE` branch reading a bare column makes the expression per-row, and the grouping rejects it.

## `HAVING`

`HAVING` filters the already computed groups, so it is written over an aggregate:

```rust
use rurqe::prelude::*;

let built = select_from::<Postgres<Pg16>, Order>()
    .group_by(OrderUserId)
    .select((OrderUserId, count_all()))
    .having(sum(OrderTotal).gt(0.0f64))
    .build()
    .expect("valid query");
```

```text
SELECT "user_id", COUNT(*) FROM "orders" GROUP BY "user_id" HAVING (SUM("total") > $1)
```

Two kinds of predicate are allowed (`HavingPredicate`):

- an aggregate one (`Marker: AggregateMarker`) — as above;
- a constant within the group (`Expr<bool, (), NonAggregate, ..>`) — for example a `sql!` fragment comparing two aggregates.

A predicate reading a bare column is rejected: the grouping has discarded the per-row value. A separate form exists for comparing a **grouped** column:

```rust
use rurqe::postgres::prelude::*;

let filtered = select_from(Orders)
    .group_by(OrderUserId)
    .select((OrderUserId, count_all()))
    .having_col(OrderUserId, Expr::column::<OrderUserId>().gt(10i64))
    .build()?;
```

`having` is available only on a `GroupedQuery` — a plain `SelectQuery` has no `HAVING` clause.

## Ordering and paginating groups

| Method | What it orders |
|---|---|
| `order_by` / `order_by_asc` / `order_by_desc` / `order_by_nulls` | a grouped column (checked by `ColumnInGroupBy`) |
| `order_by_expr` / `order_by_expr_asc` / `order_by_expr_desc` / `order_by_expr_*_nulls_*` | an aggregate (`Marker: AggregateMarker`) |
| `limit` / `offset` | the number of groups |

```rust
use rurqe::postgres::prelude::*;

let top = select_from(Orders)
    .group_by(OrderUserId)
    .select((OrderUserId, count_all()))
    .order_by_expr_desc(count_all())
    .limit(10)
    .build()?;
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

- [Window functions](window.md) — a window over groups.
- [Projections and `SELECT`](select.md)
- [NF² and MULTISET](../theory/nf2_multiset.md) — an aggregated subquery as a derived table.
