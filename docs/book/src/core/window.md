# Window functions

A window expression is computed by the engine with an `OVER (...)` clause, and it belongs to the `WINDOW` step of the logical query processing order: after the grouping, before the projection.

Every window carries three things the compiler checks:

- the relations the expression and its specification read;
- the columns the window reads, which a grouping checks like any other reference;
- the dialect requirements, including `NULLS FIRST` / `NULLS LAST` inside the window's own `ORDER BY`.

## The specification

The constructors live on `Window`, and every one of them returns a specification that the same methods extend:

| Builder | Produces |
|---|---|
| `Window::new()` / `Window::empty()` | `OVER ()` |
| `Window::partition_by(partition)` | `OVER (PARTITION BY ...)` |
| `Window::order_by(col, direction)` / `order_by_asc(col)` / `order_by_desc(col)` | `OVER (ORDER BY col ASC \| DESC)` |
| `Window::order_by_nulls(col, direction, nulls)` and its four shortcuts | the same, with `NULLS FIRST` or `NULLS LAST` |
| `Window::rows_between(start, end)` / `range_between(start, end)` | `OVER (ROWS BETWEEN ... AND ...)`, `OVER (RANGE BETWEEN ... AND ...)` |
| `.partition_by`, `.order_by*`, `.rows_between`, `.range_between` on a specification | the specification, extended |

Every call merges the relations, the columns, and the dialect requirements of the specification it receives.

## Functions

| Function | Result type |
|---|---|
| `row_number()` | `i64` |
| `rank()` | `i64` |
| `dense_rank()` | `i64` |
| `lag(col)` | `Option<T>`, where `T` is the column's type |
| `lead(col)` | `Option<T>`, where `T` is the column's type |

`LAG` / `LEAD` take an `.offset(n)`; without it the offset defaults to 1. A ranking window reads no column operand, while the column handed to `lag` / `lead` joins the columns the window reads.

The window is obtained by calling `.over(spec)`.

## Example

```rust,no_run
{{#rustdoc_include ../../../../examples/window.rs:ranked}}
```

```text
SELECT "id", ROW_NUMBER() OVER (PARTITION BY "department_id" ORDER BY "salary" DESC)
FROM "employees"
```

## Window frames

| Bound | Meaning |
|---|---|
| `WindowFrameBound::UnboundedPreceding` | the frame starts at the first row of the partition |
| `WindowFrameBound::preceding(n)` | `n` rows (or values) above the current row |
| `WindowFrameBound::CurrentRow` | the current row |
| `WindowFrameBound::following(n)` | `n` rows (or values) below the current row |
| `WindowFrameBound::UnboundedFollowing` | the frame ends at the last row of the partition |

```rust,no_run
{{#rustdoc_include ../../../../examples/window.rs:frames}}
```

`ROWS` counts the bounds in physical rows, `RANGE` — in values of the ordering column (steadier with coinciding timestamps). `GROUPS` is not implemented: it is not supported by SQLite and is covered by `ROWS`/`RANGE`.

## Invariants

1. **Capability.** A statement projecting a window compiles only if the dialect supports window functions — PostgreSQL always, SQLite from 3.25.
2. **Scope.** The `PARTITION BY`, the window's `ORDER BY`, and the `lag`/`lead` column must belong to the query's tables — otherwise the error lands at the call site.
3. **`NULLS FIRST` / `NULLS LAST` inside a window** requires a dialect that supports the placement — PostgreSQL always, SQLite from 3.30; the requirement travels with the window expression up to the statement.
4. **Grouping.** The columns a window reads are checked like any other reference, and its operand either aggregates the rows of the group or stays constant.
