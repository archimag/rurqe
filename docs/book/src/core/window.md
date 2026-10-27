# Window functions

A window expression is computed by the engine with an `OVER (...)` clause. In RURQE every window carries three things the compiler checks:

- `Scope` — the tables the expression and its specification read;
- `Cols` — the type-level set of columns the window reads (`GROUP BY` checks the window through it);
- `D` — the dialect requirements (including `NULLS FIRST/LAST` inside the window's `ORDER BY`).

## The specification

```rust
impl Window {
    pub fn new() -> WindowSpec<()>;
    pub fn empty() -> WindowSpec<()>;
    pub fn partition_by<P: PartitionBy>(partition: P) -> WindowSpec<P::Scope, P::Columns>;
    pub fn order_by<C: Column>(col: C, direction: OrderDirection) -> WindowSpec<C::Table, WindowColumnSet<C>>;
    pub fn order_by_asc<C: Column>(col: C) -> WindowSpec<C::Table, WindowColumnSet<C>>;
    pub fn order_by_desc<C: Column>(col: C) -> WindowSpec<C::Table, WindowColumnSet<C>>;
    pub fn order_by_nulls<C: Column>(col: C, direction: OrderDirection, nulls: NullsOrder) -> /* + RequiresNullsOrdering */;
    pub fn rows_between(start: WindowFrameBound, end: WindowFrameBound) -> WindowSpec<()>;
    pub fn range_between(start: WindowFrameBound, end: WindowFrameBound) -> WindowSpec<()>;
}
```

`WindowSpec` is extended by the same methods (`partition_by`, `order_by*`, `rows_between`, `range_between`); every call merges the scopes, the column sets, and the dialect requirements.

## Functions

| Function | Result type |
|---|---|
| `row_number()` | `i64` |
| `rank()` | `i64` |
| `dense_rank()` | `i64` |
| `lag(col)` | `Option<col::RustType>` |
| `lead(col)` | `Option<col::RustType>` |

`LAG`/`LEAD` take an `.offset(n)`; without it the offset defaults to 1. The zero-marker of a window without an operand is `Aggregate` (the identity of the marker fold), and the `lag`/`lead` column lands in the `Cols` set.

The window is obtained by calling `.over(spec)`:

```rust
pub fn over<WinScope, WinCols, WinD>(self, window: WindowSpec<WinScope, WinCols, WinD>)
    -> WindowExpr<T, WinScope, WinCols, WinD, /* operand markers */>;
```

## Example

```rust
use rurqe::postgres::prelude::*;

let ranked = select_from(Employees)
    .select((
        EmployeeId,
        row_number().over(Window::new().partition_by(DepartmentId).order_by_desc(Salary)),
    ))
    .build()?;
```

```text
SELECT "id", ROW_NUMBER() OVER (PARTITION BY "department_id" ORDER BY "salary" DESC) FROM "employees"
```

A window over groups:

```rust
use rurqe::postgres::prelude::*;

let aggregated = select_from(Orders)
    .group_by(OrderUserId)
    .select((
        OrderUserId,
        Expr::sum::<OrderUserId>().over(Window::partition_by(OrderUserId)),
    ))
    .build()
    .expect("an aggregate operand keeps one value per group");
```

```text
SELECT "user_id", SUM("user_id") OVER (PARTITION BY "user_id") FROM "orders" GROUP BY "user_id"
```

## Window frames

```rust
pub enum WindowFrameKind { Rows, Range }

pub enum WindowFrameBound {
    UnboundedPreceding,
    Preceding(u32),
    CurrentRow,
    Following(u32),
    UnboundedFollowing,
}
```

`WindowFrameBound::preceding(n)` / `::following(n)` are constructors; the rest are enum variants.

```rust
use rurqe::postgres::prelude::*;

let moving = select_from(Orders)
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
```

`ROWS` counts the bounds in physical rows, `RANGE` — in values of the ordering column (steadier with coinciding timestamps). `GROUPS` is not implemented: it is not supported by SQLite and is covered by `ROWS`/`RANGE`.

## Invariants

1. **Capability.** A statement projecting a window compiles only if the dialect implements `SupportsWindowFunctions`.
2. **Scope.** The `PARTITION BY`, the window's `ORDER BY`, and the `lag`/`lead` column must belong to the query's tables — otherwise the error lands at the call site.
3. **`NULLS FIRST/LAST` inside a window** requires `SupportsNullsOrdering`; the requirement travels with the window expression up to the statement.
4. **Grouping.** The window columns are checked through `WindowColsInGroupBy`, and the operand through the aggregate marker: `SUM("user_id")` folds one value per group, so a window over an aggregate stays projectable from a `GroupedQuery` — the example above.
