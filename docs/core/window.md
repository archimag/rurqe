# Window functions

A window expression is computed by the engine with an `OVER (...)` clause, and it belongs to the `WINDOW` step of the logical query processing order: after the grouping, before the projection.

Every window carries three things the compiler checks:

- the relations the expression and its specification read;
- the columns the window reads, which a grouping checks like any other reference;
- the dialect requirements, including `NULLS FIRST` / `NULLS LAST` inside the window's own `ORDER BY`.

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
4. **Grouping.** The columns a window reads are checked like any other reference, and its operand either aggregates the rows of the group or stays constant.
