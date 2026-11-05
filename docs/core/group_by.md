# `GROUP BY` and `HAVING`

`GROUP BY` folds the source rows into groups, and `HAVING` filters the groups the fold produced. The grouping is the third step of the logical query processing order: it runs after `WHERE` has chosen the source rows and before the projection is evaluated.

That order is what makes the rule "a column is either grouped or aggregated" checkable at all: the grouping is known before the projection is written, so a projection naming a column the grouping collapsed is refused at the call site. Writing `.select(...)` first would leave the projection nothing to check it against.

```rust,no_run
{{#rustdoc_include ../examples/aggregates.rs:group_by}}
```

```sql
SELECT "users"."id" AS "user_id", COUNT(*) AS "orders"
FROM "users" INNER JOIN "orders" ON ("orders"."user_id" = "users"."id")
GROUP BY "users"."id"
HAVING (COUNT(*) > $1)
```

A single column or a tuple can be grouped: `.group_by((UserId, UserName))`.

## What can be projected

Exactly five projection forms survive a grouping:

| Projection | Condition |
|---|---|
| a column | it is part of `GROUP BY` |
| an aggregate expression | the call folds the rows of the group |
| a constant expression | it reads no table |
| a computed expression | it is equal to a grouped expression, which the statement build checks |
| a window expression | the window reads grouped columns, and so do the columns its operand aggregates |

Projecting an ungrouped column marker is rejected at the call site, for `select_from(...).select(...)` and for a `select!` selection alike.

> **Compiler Guard in Action.** `OrderTotal` is not part of `GROUP BY` and is not aggregated — `select((OrderUserId, OrderTotal))` does not compile, with a message saying the column is "neither grouped nor an aggregate".
An expression built over a column keeps that column's relation, but the call site cannot tell it from an expression computed over several columns. Such a projection is therefore accepted by the compiler and refused when the statement is built, as `UngroupedColumn`, unless it equals a grouped expression (see below). `outer(...)` does not help either — it drops the relation from the expression but does not turn the expression into a constant, so the grouping still refuses it.

## Grouping by a computed expression

`group_by` accepts a per-row expression in the same position as a column. Its tables are checked against the statement like those of any other clause, and an aggregate is refused at compile time because SQL forbids an aggregate in `GROUP BY`. The example computes the day of a timestamp with a PostgreSQL function written once through operand fragments:

```rust,no_run
{{#rustdoc_include ../examples/aggregates.rs:heat_map_definition}}
```

The projection and the grouping use the same expression, and the statement counts the rows of each day:

```rust,no_run
{{#rustdoc_include ../examples/aggregates.rs:heat_map}}
```

```sql
SELECT date_trunc('day', "created_at") AS "day", COUNT(*) AS "visits"
FROM "visits"
GROUP BY date_trunc('day', "created_at")
```

The rules of a grouped expression:

- The columns inside a grouped expression do not become grouped columns. A bare `VisitCreatedAt` is not projectable beside `day_of(VisitCreatedAt)`.
- A projection or a `HAVING` predicate reads the expression only as an expression structurally equal to the grouped one. The comparison runs inside `.build()` (Tier 2), because it compares the assembled expressions. A variation — another unit, another column — is refused as `UngroupedExpression`, and a bare column as `UngroupedColumn`.
- A single `group_by` call takes a column, a tuple of columns, or one expression. Several groupings chain: `.group_by(UserId).group_by(day_of(VisitCreatedAt))`.
- Write a constant of the grouped expression as literal text in the fragment. A value bound as a parameter is numbered per occurrence, so the projected copy and the grouped copy are rendered with different placeholders.

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

Two kinds of predicate are allowed here:
- an aggregate predicate, which folds the rows of the group — as above;
- a constant within the group, for example a `sql!` fragment comparing two aggregates.

A predicate reading a bare column is rejected: the grouping has discarded the per-row value. A separate form exists for comparing a **grouped** column:

```rust,no_run
{{#rustdoc_include ../examples/aggregates.rs:having_col}}
```

`having` is available only on a grouped statement — a plain query has no `HAVING` clause.

## `FILTER (WHERE ...)`

The rows an aggregate call evaluates can be restricted without a `CASE WHEN` detour: the clause is written on the call itself and travels with the expression wherever the caller projects it.

```rust,no_run
{{#rustdoc_include ../examples/aggregates.rs:filtered}}
```

`filter_where` obeys the same belonging rule as the call's `DISTINCT` modifier:

* the method exists on the aggregate call alone — an expression built from the call, such as `sum(OrderTotal) + 1.0`, is an aggregate but no longer the call, so it accepts no clause: there is nothing left for the clause to restrict;
* repeated calls accumulate their predicates with `AND` on the same call instead of replacing each other;
* the predicate stays part of the aggregate expression rather than of the statement, and the rendering is the native `... FILTER (WHERE ...)` form — PostgreSQL always, SQLite from 3.30.

## Ordering and paginating groups

| Method | What it orders |
|---|---|
| `order_by` / `order_by_asc` / `order_by_desc` / `order_by_nulls` | a grouped column |
| `order_by_expr` / `order_by_expr_asc` / `order_by_expr_desc` / `order_by_expr_*_nulls_*` | an aggregate expression |
| `limit` / `offset` | the number of groups |

```rust,no_run
{{#rustdoc_include ../examples/aggregates.rs:group_ordering}}
```

## Finishing a grouped query

- `.build()` — only after `.select(...)`;
- `.as_cte::<T>()` and `.as_table::<T>()` — nest the aggregated query as a CTE or a derived table;
- a further `.group_by(...)` is possible only before the projection.

A grouped query cannot lock its rows: `for_update` / `for_share` do not exist on it, because PostgreSQL does not lock the rows of a statement that yields one row per group.

## What `.build()` adds

The call site refuses a grouped query that reads a column the grouping dropped — in the projection, in the ordering, in a window specification, and in a `HAVING` predicate. `.build()` checks the same rules again on the assembled statement, and for one family of statements that check is the only one there is: a query that never grouped its rows carries no grouping state at all, so nothing in the builder knows that an aggregate collapses the relation.

| Typed failure | The statement it refuses |
|---|---|
| `UngroupedColumnWithAggregate` | a query without `GROUP BY` that projects an aggregate beside a bare column |
| `UngroupedColumn` | a statement whose rows an aggregate collapsed, read through a column the collapse dropped — in a projection, in the ordering, in a `DISTINCT ON` expression, or in the operand of a window call |
| `UngroupedHavingColumn` | a `HAVING` predicate reading a column that is neither aggregated nor grouped |
| `UngroupedExpression` | a statement grouped by a computed expression whose projection or ordering is computed over columns that no grouped expression covers |

`select_from(Order).select((OrderUserId, count_all()))` is the first row: it compiles, and the statement is refused when it is built, with a diagnostic asking for the grouping this chapter describes. The second row also governs the ordering of an aggregated query — projecting `count_all()` and ordering the result by `OrderTotal` names a per-row column the collapse removed — while a grouped query orders by a grouped column or by an aggregate, which is what its ordering methods accept.

The operand of a window call is covered by the same walk. A window evaluated over the rows a statement produced reads that operand from them, so the columns an aggregate inside the operand aggregates have to survive the grouping exactly like the argument of a statement aggregate — the walk collects them through the scalar functions, the arithmetic operators, a `CASE` branch, and a `CAST` around the call. An aggregate nested inside the operand — `sum(sum(OrderTotal)) OVER (...)` — collapses the groups before the window reads them, so it ends that walk and governs the column it reads itself.