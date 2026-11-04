# Window functions

A window expression is computed by the engine with an `OVER (...)` clause, and it belongs to the `WINDOW` step of the logical query processing order: after the grouping, before the projection.

Every window carries four things the library checks:

- the relations the expression and its specification read;
- the columns the window reads, which a grouping checks like any other reference;
- the dialect requirements, including `NULLS FIRST` / `NULLS LAST` inside the window's own `ORDER BY`;
- the operand the clause attaches to, which has to be a function application.

## The specification

The constructors live on `Window`, and every one of them returns a specification that the same methods extend:

| Builder | SQL |
|---|---|
| `Window::new()` / `Window::empty()` | `OVER ()` |
| `Window::partition_by(col)` | `OVER (PARTITION BY col)` |
| `Window::order_by(col, direction)` | `OVER (ORDER BY col ASC \| DESC)` |
| `Window::order_by_asc(col)` / `Window::order_by_desc(col)` | `OVER (ORDER BY col ASC)` / `OVER (ORDER BY col DESC)` |
| `Window::order_by_nulls(col, direction, nulls)` | `OVER (ORDER BY col ... NULLS FIRST \| LAST)` |
| `Window::rows_between(start, end)` | `OVER (ROWS BETWEEN ... AND ...)` |
| `Window::range_between(start, end)` | `OVER (RANGE BETWEEN ... AND ...)` |

Every builder method is also available on `WindowSpec` to extend an existing specification. Chaining methods merges the relations, columns, and dialect requirements into the resulting specification.

## The operand

`OVER (...)` attaches to a function application and to nothing else. The builder accepts three operand shapes:

- an aggregate call — `sum(OrderTotal)`, `count_all()`, or `avg(OrderTotal)`;
- a scalar function call — `coalesce(sum(OrderTotal), 0.0)`;
- a raw fragment trusted to spell a call — `sql!(<i64> "COUNT(*)")`.

Every other shape names a value rather than a call: appending the clause to it would render SQL no engine parses — a column reference yields `"total" OVER (...)`, a bound value yields `$1 OVER ()`, and a subquery, an operator expression, a `CAST`, or a `CASE` yields its own form followed by the clause. Such a statement is refused while it is built, before any SQL reaches the engine:

```rust,no_run
{{#rustdoc_include ../examples/window.rs:operand_refusal}}
```

```text
window expression is applied to a column reference, which is not a function call;
an `OVER` clause attaches only to a function application,
so apply it to an aggregate such as `sum(...)`,
to a scalar function such as `coalesce(...)`,
or to a raw fragment that spells the call
```

A scalar function is a function application of its own, which is why `coalesce(sum(OrderTotal), 0.0) OVER (...)` stays valid: the operand renders as `COALESCE(SUM("total"), $1) OVER (...)`, and the columns the aggregate reads inside it stay subject to the grouping.

## Functions

| Function | SQL | Result type |
|---|---|---|
| `row_number()` | `ROW_NUMBER()` | `i64` |
| `rank()` | `RANK()` | `i64` |
| `dense_rank()` | `DENSE_RANK()` | `i64` |
| `lag(col)` | `LAG(col)` | `Option<T>`, where `T` is the column's type |
| `lead(col)` | `LEAD(col)` | `Option<T>`, where `T` is the column's type |

`LAG` / `LEAD` take an `.offset(n)`; without it the offset defaults to 1. A ranking window reads no column operand, while the column handed to `lag` / `lead` joins the columns the window reads.

The window is obtained by calling `.over(spec)`.

## Example

```rust,no_run
{{#rustdoc_include ../examples/window.rs:ranked}}
```

```sql
SELECT "id" AS "employee_id", ROW_NUMBER() OVER (PARTITION BY "department_id" ORDER BY "salary" DESC) AS "position"
FROM "employees"
```

## Window frames

- `WindowFrameBound::UnboundedPreceding`: the frame starts at the first row of the partition.
- `WindowFrameBound::preceding(n)`: `n` rows (or values) preceding the current row.
- `WindowFrameBound::CurrentRow`: the current row.
- `WindowFrameBound::following(n)`: `n` rows (or values) following the current row.
- `WindowFrameBound::UnboundedFollowing`: the frame ends at the last row of the partition.

```rust,no_run
{{#rustdoc_include ../examples/window.rs:frames}}
```

```sql
SELECT "id" AS "order_id", SUM("total") OVER (ORDER BY "created_at" ASC ROWS BETWEEN 2 PRECEDING AND CURRENT ROW) AS "running_total"
FROM "orders"
```

`ROWS` counts the bounds in physical rows, `RANGE` — in values of the ordering column (steadier with coinciding timestamps). `GROUPS` is not implemented: it is not supported by SQLite and is covered by `ROWS`/`RANGE`.

## Invariants

1. **Capability.** A statement projecting a window compiles only if the dialect supports window functions — PostgreSQL always, SQLite from 3.25.
2. **Scope.** The `PARTITION BY`, the window's `ORDER BY`, and the `lag`/`lead` column must belong to the query's tables — otherwise the error lands at the call site.
3. **`NULLS FIRST` / `NULLS LAST` inside a window** requires a dialect that supports the placement — PostgreSQL always, SQLite from 3.30; the requirement travels with the window expression up to the statement.
4. **Grouping.** The columns a window reads are checked like any other reference, and its operand either aggregates the rows of the group or stays constant. The columns the aggregate inside the operand reads are covered by the grouping as well — the walk collects them through the scalar functions, the arithmetic operators, a `CASE`, and a `CAST` around the call.
5. **Operand.** An `OVER (...)` clause attaches to a function application — an aggregate, a scalar function call, or a raw fragment that spells the call. Every other operand shape is refused while the statement is built, with a diagnostic naming the construct.
