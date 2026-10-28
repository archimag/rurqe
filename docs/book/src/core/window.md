# Window functions

A window expression is computed by the engine with an `OVER (...)` clause. In RURQE every window carries three things the compiler checks:

- `Scope` — the tables the expression and its specification read;
- `Cols` — the type-level set of columns the window reads (`GROUP BY` checks the window through it);
- `D` — the dialect requirements (including `NULLS FIRST/LAST` inside the window's `ORDER BY`).

## The specification

The constructors live on `Window`, and every one of them returns a specification that the same methods extend:

| Builder | Produces |
|---|---|
| `Window::new()` / `Window::empty()` | `WindowSpec<()>` — `OVER ()` |
| `Window::partition_by(partition)` | `WindowSpec<P::Scope, P::Columns>` |
| `Window::order_by(col, direction)` / `order_by_asc(col)` / `order_by_desc(col)` | `WindowSpec<C::Table, WindowColumnSet<C>>` |
| `Window::order_by_nulls(col, direction, nulls)` and its four shortcuts | the same, requiring `SupportsNullsOrdering` |
| `Window::rows_between(start, end)` / `range_between(start, end)` | a framed `WindowSpec<()>` |
| `.partition_by`, `.order_by*`, `.rows_between`, `.range_between` on a `WindowSpec` | the specification, extended |

Every call merges the scopes, the column sets, and the dialect requirements of the specification it receives.

## Functions

| Function | Result type |
|---|---|
| `row_number()` | `i64` |
| `rank()` | `i64` |
| `dense_rank()` | `i64` |
| `lag(col)` | `Option<col::RustType>` |
| `lead(col)` | `Option<col::RustType>` |

`LAG`/`LEAD` take an `.offset(n)`; without it the offset defaults to 1. The zero-marker of a window without an operand is `Aggregate` (the identity of the marker fold), and the `lag`/`lead` column lands in the `Cols` set.

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

1. **Capability.** A statement projecting a window compiles only if the dialect implements `SupportsWindowFunctions`.
2. **Scope.** The `PARTITION BY`, the window's `ORDER BY`, and the `lag`/`lead` column must belong to the query's tables — otherwise the error lands at the call site.
3. **`NULLS FIRST/LAST` inside a window** requires `SupportsNullsOrdering`; the requirement travels with the window expression up to the statement.
4. **Grouping.** The window columns are checked through `WindowColsInGroupBy`, and the operand through the aggregate marker: `SUM("user_id")` folds one value per group, so a window over an aggregate stays projectable from a `GroupedQuery` — the example above.
