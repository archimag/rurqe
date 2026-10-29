# Window functions

A window expression is computed by the engine with an `OVER (...)` clause, and it belongs to the `WINDOW` step of the [logical query processing order](select.md#logical-query-processing-order): after the grouping, before the projection. The head page of the statement family is [The `SELECT` statement](select.md).

Every window carries three things the compiler checks:

- the relations the expression and its specification read;
- the columns the window reads, which a grouping checks like any other reference;
- the dialect requirements, including `NULLS FIRST` / `NULLS LAST` inside the window's `ORDER BY`.

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
| `lag(col)` | `Option<col::RustType>` |
| `lead(col)` | `Option<col::RustType>` |

`LAG` / `LEAD` take an `.offset(n)`; without it the offset defaults to 1. A ranking window reads no column operand, while the column handed to `lag` / `lead` joins the columns the window reads.

The window is obtained by calling `.over(spec)`.

## Example

```rust,no_run
# extern crate rurqe;
# use rurqe::postgres::prelude::*;
# table! { Employee => "employees" { EmployeeId => "id": i64, DepartmentId => "department_id": i64, Salary => "salary": i64, } }
# fn main() -> Result<(), Box<dyn std::error::Error>> {
let ranked = select_from(Employee)
    .select((
        EmployeeId,
        row_number().over(Window::new().partition_by(DepartmentId).order_by_desc(Salary)),
    ))
    .build()?;
# let _ = ranked;
# Ok(())
# }
```

```text
SELECT "id", ROW_NUMBER() OVER (PARTITION BY "department_id" ORDER BY "salary" DESC) FROM "employees"
```

A window over groups:

```rust,no_run
# extern crate rurqe;
# use rurqe::postgres::prelude::*;
# table! { Order => "orders" { OrderUserId => "user_id": i64, } }
# fn main() {
let aggregated = select_from(Order)
    .group_by(OrderUserId)
    .select((
        OrderUserId,
        Expr::sum::<OrderUserId>().over(Window::partition_by(OrderUserId)),
    ))
    .build()
    .expect("an aggregate operand keeps one value per group");
# let _ = aggregated;
# }
```

```text
SELECT "user_id", SUM("user_id") OVER (PARTITION BY "user_id") FROM "orders" GROUP BY "user_id"
```

## Window frames

```rust
# extern crate rurqe;
# use rurqe::core::{WindowFrameBound, WindowFrameKind};
pub enum WindowFrameKind { Rows, Range }

pub enum WindowFrameBound {
    UnboundedPreceding,
    Preceding(u32),
    CurrentRow,
    Following(u32),
    UnboundedFollowing,
}
# fn main() {}
```

`WindowFrameBound::preceding(n)` / `::following(n)` are constructors; the rest are enum variants.

```rust,no_run
# extern crate rurqe;
# use rurqe::postgres::prelude::*;
# table! { Order => "orders" { OrderId => "id": i64, OrderTotal => "total": f64, OrderCreatedAt => "created_at": i64, } }
# fn main() -> Result<(), Box<dyn std::error::Error>> {
let moving = select_from(Order)
    .select((
        OrderId,
        Expr::sum::<OrderTotal>().over(
            Window::order_by_asc(OrderCreatedAt).rows_between(
                WindowFrameBound::preceding(2),
                WindowFrameBound::CurrentRow,
            ),
        ),
    ))
    .build()?;
# let _ = moving;
# Ok(())
# }
```

`ROWS` counts the bounds in physical rows, `RANGE` — in values of the ordering column (steadier with coinciding timestamps). `GROUPS` is not implemented: it is not supported by SQLite and is covered by `ROWS`/`RANGE`.

## Invariants

1. **Capability.** A statement projecting a window compiles only if the dialect supports window functions — PostgreSQL always, SQLite from 3.25.
2. **Scope.** The `PARTITION BY`, the window's `ORDER BY`, and the `lag`/`lead` column must belong to the query's tables — otherwise the error lands at the call site.
3. **`NULLS FIRST` / `NULLS LAST` inside a window** requires a dialect that supports the placement — PostgreSQL always, SQLite from 3.30; the requirement travels with the window expression up to the statement.
4. **Grouping.** The columns a window reads are checked like any other reference, and its operand either aggregates the rows of the group or stays constant: `SUM("user_id")` folds one value per group, so a window over an aggregate stays projectable from a grouped statement — the example above.

## Related chapters

- [The `SELECT` statement](select.md) — the clause anatomy and the evaluation order this chapter is one step of.
- [`GROUP BY` and `HAVING`](group_by.md) — a window over groups.
- [Expressions](expressions.md) — arithmetic, `CASE`, and the scalar functions.
- [Ordering and pagination](ordering.md) — the same ordering rules outside a window.
