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

`group_by` accepts a per-row expression in the same position as a column. Its tables are checked against the statement like those of any other clause, and an aggregate is refused at compile time because SQL forbids an aggregate in `GROUP BY`. The example buckets a timestamp by its calendar day with the PostgreSQL `date_trunc` builder, which keeps the Rust type and the scope of the column it truncates:

```rust,no_run
{{#rustdoc_include ../examples/aggregates.rs:heat_map}}
```

```sql
SELECT date_trunc('day', "created_at") AS "day", COUNT(*) AS "visits"
FROM "visits"
GROUP BY date_trunc('day', "created_at")
ORDER BY date_trunc('day', "created_at") ASC
```

The projection, the grouping, and the ordering name the same expression three times, and the three copies render the same text: the builder writes the unit into the statement as a literal rather than binding it as a parameter, so no copy is numbered differently from another. A function the typed surface does not model is written through operand fragments, and the rules below apply to it the same way.

The rules of a grouped expression:

- The columns inside a grouped expression do not become grouped columns. A bare `VisitCreatedAt` is not projectable beside `VisitCreatedAt.date_trunc(PgDateTruncUnit::Day)`.
- A projection, a `HAVING` predicate, or an ordering criterion reads the expression only as an expression structurally equal to the grouped one. The comparison runs inside `.build()` (Tier 2), because it compares the assembled expressions. A variation — another unit, another column — is refused as `UngroupedExpression`, and a bare column as `UngroupedColumn`.
- A single `group_by` call takes a column, a tuple of columns, or one expression. Several groupings chain: `.group_by(UserId).group_by(VisitCreatedAt.date_trunc(PgDateTruncUnit::Day))`.
- Keep every constant of a computed call literal. A value bound as a parameter is numbered per occurrence, so the projected copy and the grouped copy would be rendered with different placeholders; the `date_trunc` builder writes its unit as literal text for exactly that reason, and a call written through operand fragments follows the same rule.

## The aggregate functions

| Call | SQL | Operand |
|---|---|---|
| `count_all()` | `COUNT(*)` | none |
| `count(col)` | `COUNT(col)` | a column marker |
| `count_distinct(operand)` | `COUNT(DISTINCT operand)` | a column or a per-row expression |
| `sum(operand)` / `sum_distinct(operand)` | `SUM(operand)` / `SUM(DISTINCT operand)` | a column or a per-row expression |
| `avg(operand)` / `avg_distinct(operand)` | `AVG(operand)` / `AVG(DISTINCT operand)` | a column or a per-row expression |
| `min(operand)` / `max(operand)` | `MIN(operand)` / `MAX(operand)` | a column or a per-row expression |

`count_all()` counts rows; `count(col)` counts the rows where the column is present — the distinction the two spellings carry.

An aggregate call is an expression like any other: it compares — `.having(count_all().gt(0i64))` above — and it travels through the arithmetic and the scalar functions. The call-attached `DISTINCT` modifier and `FILTER (WHERE ...)` exist on the call alone; an expression built from the call, such as `sum(OrderTotal) + 1.0`, accepts neither.

## Aggregates over expressions

Every aggregate except `count_all()` and `count(col)` accepts a per-row expression in place of a column, which is what the `Operand` column of the table above states. The operand is anything that computes one value per row — a column marker, arithmetic over columns, a scalar function, a `CASE` branch, a call written through operand fragments, or a `sql!` fragment — and the call keeps the tables the operand reads, so an operand referencing a relation the statement does not introduce is rejected at the call site exactly like the bare column.

```rust,no_run
{{#rustdoc_include ../examples/aggregates.rs:aggregate_expressions}}
```

```sql
SELECT "user_id", SUM(ROUND("total", 2)), SUM(COALESCE(total, 0)) FROM "orders" GROUP BY "user_id"
```

The call renders its operand exactly as the operand renders on its own: `SUM(ROUND("total", 2))` adds the function name and the parentheses around it and nothing else, and the parameters the operand binds are numbered before the ones of a `FILTER (WHERE ...)` clause written on the same call.

An operand reads the rows of a group, so a call is not an operand of another call: `sum(sum(OrderTotal))` does not compile. That is the type-level counterpart of the refusal every engine states for a nested aggregate, because the inner call leaves no row for the outer one to read. One row per group aggregated again is the window form — `SUM(SUM("total")) OVER (...)` — which the window builder composes.

> **Compiler Guard in Action.** `sum(sum(OrderTotal))` does not compile, with a message saying the inner call is "not an operand of an aggregate function".

A value whose Rust type the typed surface does not name stays in trusted text. PostgreSQL `INTERVAL` is the case that matters for analytics: no Rust type maps onto an interval column, so a cell computed over one is spelled `sum(sql!(<f64> "..."))` — the second projection above spells a value the builder does not model that way — and the trusted text is where the interval arithmetic itself lives.

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
| `order_by_grouped_expr` / `order_by_grouped_expr_asc` / `order_by_grouped_expr_desc` / `order_by_grouped_expr_*_nulls_*` | the grouped computed expression itself |
| `limit` / `offset` | the number of groups |

```rust,no_run
{{#rustdoc_include ../examples/aggregates.rs:group_ordering}}
```
A statement grouped by a computed expression orders its groups by the same expression it grouped by: `order_by_grouped_expr*` takes the per-row expression, and the criterion has to be structurally equal to a grouped one. The comparison runs while the statement is built, exactly like the one the projection follows — a variation is refused as `UngroupedExpression`, and a bare column the grouping dropped as `UngroupedColumn`.

The `*_nulls_*` variants of the two expression families, and `order_by_nulls` for a grouped column, place absent values explicitly and are available on a dialect that supports the placement.
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