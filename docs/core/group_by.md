# `GROUP BY` and `HAVING`

`GROUP BY` folds the source rows into groups, and `HAVING` filters the groups the fold produced. The grouping is the third step of the logical query processing order: it runs after `WHERE` has chosen the source rows and before the projection is evaluated.

That order is what makes the rule "a column is either grouped or aggregated" checkable at all: the grouping is known before the projection is written, so a projection naming a column the grouping collapsed is refused at the call site. Writing `.select(...)` first would leave the projection nothing to check it against.

```rust,no_run
{{#rustdoc_include ../examples/aggregates.rs:group_by}}
```

```sql
SELECT "users"."id" AS "user_id", COUNT(*)
FROM "users" INNER JOIN "orders" ON ("orders"."user_id" = "users"."id")
GROUP BY "users"."id"
HAVING (COUNT(*) > $1)
```

A single column or a tuple can be grouped: `.group_by((UserId, UserName))`.

## What can be projected

Exactly four projection forms survive a grouping:

| Projection | Condition |
|---|---|
| a column | it is part of `GROUP BY` |
| an aggregate expression | the call folds the rows of the group |
| a constant expression | it reads no table |
| a window expression | the window reads grouped columns, and its operand aggregates rows or is constant |

Projecting an ungrouped column is rejected at the call site, for `select_from(...).select(...)` and for a `select!` selection alike.

The low-level `Expr::column::<C>()` form does not bypass the rule: an expression reading a column keeps that column's relation. `.without_scope()` does not help either — it drops the relation from the expression but does not turn the expression into a constant, so the grouping still refuses it.

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

An aggregate call is an expression like any other: it compares — `.having(count_all().gt(0i64))` above — and it travels through the arithmetic and the scalar functions. The call-attached `DISTINCT` modifier and `FILTER (WHERE ...)` exist on the call alone; an expression built from the call, such as `sum(OrderTotal) + 1.0`, accepts neither.

## Aggregate folding in scalar functions

`coalesce`, `nullif`, `greatest`, `least`, and `CASE` fold the aggregation of their arguments, so `coalesce(sum(orders.total), 0.0)` stays an aggregate and projects from a grouped statement. A `CASE` branch reading a bare column makes the whole expression per-row, and the grouping rejects it.

## `HAVING`

`HAVING` filters the already computed groups, so it is written over an aggregate. The example above already uses `.having(count_all().gt(0i64))`; it renders as `HAVING (COUNT(*) > $1)`.

Two kinds of predicate are allowed (`HavingPredicate`):
Two kinds of predicate are allowed here:
- an aggregate one (`Marker: AggregateMarker`) — as above;
- a constant within the group (`Expr<bool, (), NonAggregate, ..>`) — for example a `sql!` fragment comparing two aggregates.

A predicate reading a bare column is rejected: the grouping has discarded the per-row value. A separate form exists for comparing a **grouped** column:

```rust,no_run
let filtered = select_from(Order)
    .group_by(OrderUserId)
    .select((OrderUserId, count_all()))
    .having_col(OrderUserId, Expr::column::<OrderUserId>().gt(10i64))
    .build()?;
```

`having` is available only on a grouped statement — a plain query has no `HAVING` clause.

## `FILTER (WHERE ...)`

The rows an aggregate call evaluates can be restricted without a `CASE WHEN` detour: the clause is written on the call itself and travels with the expression wherever the caller projects it.

```rust,no_run
{{#rustdoc_include ../examples/aggregates.rs:filtered}}
```

`filter_where` obeys the same belonging rule as the call's `DISTINCT` modifier:

* the method exists on the aggregate call alone — an expression built from the call, such as `sum(OrderTotal) + 1.0`, is an `Aggregate` but no longer the call, so it accepts no clause: there is nothing left for the clause to restrict;
* repeated calls accumulate their predicates with `AND` on the same call instead of replacing each other;
* the predicate stays part of the aggregate expression rather than of the statement, and the rendering is the native `... FILTER (WHERE ...)` form — PostgreSQL always, SQLite from 3.30.

## Ordering and paginating groups

| Method | What it orders |
|---|---|
| `order_by` / `order_by_asc` / `order_by_desc` / `order_by_nulls` | a grouped column |
| `order_by_expr` / `order_by_expr_asc` / `order_by_expr_desc` / `order_by_expr_*_nulls_*` | an aggregate expression |
| `limit` / `offset` | the number of groups |

```rust,no_run
let top = select_from(Order)
    .group_by(OrderUserId)
    .select((OrderUserId, count_all()))
    .order_by_expr_desc(count_all())
    .limit(10)
    .build()?;
```

## Finishing a grouped query

- `.build()` — only after `.select(...)`;
- `.as_cte::<T>()` and `.as_table::<T>()` — nest the aggregated query as a CTE or a derived table;
- a further `.group_by(...)` is possible only before the projection.

A grouped query cannot lock its rows: `for_update` / `for_share` do not exist on it, because PostgreSQL does not lock the rows of a statement that yields one row per group.

## What `.build()` adds

The residual cases that cannot be expressed as a positive assertion in types: `BuildError::UngroupedColumn`, `UngroupedHavingColumn`, `UngroupedColumnWithAggregate`.

> **Compiler Guard in Action.** `OrderTotal` is not part of `GROUP BY` and is not aggregated — `select((OrderUserId, OrderTotal))` does not compile, with a message saying the column is "neither grouped nor an aggregate".
