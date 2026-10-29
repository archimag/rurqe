# Ordering and pagination

Ordering is described by the `.order_by*` family; pagination by `.limit(u32)` and `.offset(u32)`.

Both are the last steps of the [logical query processing order](select.md#logical-query-processing-order): the rows exist, the projection has named its output columns, and the ordering is applied to what they produced. The head page of the statement family is [The `SELECT` statement](select.md).

## Columns and column tuples

A criterion is written over a single column or over a tuple of up to eight columns: a tuple unfolds into as many criteria as it has elements, so `.order_by_desc((EventCreatedAt, EventId))` renders `ORDER BY "created_at" DESC, "id" DESC`.

```rust,no_run
# extern crate rurqe;
# use rurqe::postgres::prelude::*;
# table! { User => "users" { UserId => "id": i64, UserName => "name": String, } }
# fn main() {
let built = select_from(User)
    .select(cols!(UserId, UserName))
    .where_(UserId.gt(10i64))
    .order_by_asc(UserId)
    .limit(5)
    .build()
    .expect("valid query");
# let _ = built;
# }
```

```text
SELECT "id" AS "user_id", "name" AS "user_name" FROM "users" WHERE ("id" > $1) ORDER BY "id" ASC LIMIT 5
```

The family:

| Method | SQL tail |
|---|---|
| `.order_by(cols, OrderDirection::Asc/Desc)` | `ORDER BY cols ASC` |
| `.order_by_asc` / `.order_by_desc` | shortcuts |
| `.order_by_nulls(cols, dir, NullsOrder)` | `ORDER BY ... NULLS FIRST/LAST` |
| `.order_by_asc_nulls_first` / `_nulls_last` | shortcuts |
| `.order_by_desc_nulls_first` / `_nulls_last` | shortcuts |
| `.order_by_expr*` | a criterion by expression |
| `.limit(n)` / `.offset(n)` | `LIMIT` / `OFFSET` |

`NULLS FIRST` / `NULLS LAST` is a dialect capability: the methods require `D: SupportsNullsOrdering`.

## Ordering by an expression or a window

`order_by_expr*` accepts two kinds of criterion:

- an expression — arithmetic, `CASE`, a scalar function;
- a whole window, with the `OVER (...)` inside the criterion.

```rust,no_run
# extern crate rurqe;
# use rurqe::postgres::prelude::*;
# table! { Order => "orders" { OrderUserId => "user_id": i64, } }
# fn main() -> Result<(), Box<dyn std::error::Error>> {
let built = select_from(Order)
    .select((OrderUserId,))
    .order_by_expr_desc(row_number().over(Window::partition_by(OrderUserId)))
    .build()?;
# let _ = built;
# Ok(())
# }
```

> **Compiler Guard in Action.** A criterion of a plain query is a per-row value: the statement yields one row per source row, so ordering by an aggregate is refused there. An aggregate folds the rows of a group, so it orders the result only on a grouped statement — see [`GROUP BY` and `HAVING`](group_by.md).

## Keyset pagination with tuples

A tuple of columns is a `RowExpr`, and it compares against a tuple of values as a single value (SQL:1992):

```rust,no_run
# extern crate rurqe;
# use rurqe::postgres::prelude::*;
# table! { Event => "events" { EventId => "id": i64, EventCreatedAt => "created_at": i64, EventData => "data": String, } }
# fn main() -> Result<(), Box<dyn std::error::Error>> {
# let cursor_time = 0i64;
# let cursor_id = 0i64;
let page = select_from(Event)
    .select(cols!(EventId, EventCreatedAt, EventData))
    .where_((EventCreatedAt, EventId).lt((cursor_time, cursor_id)))
    .order_by_desc((EventCreatedAt, EventId))
    .limit(50)
    .build()?;
# let _ = page;
# Ok(())
# }
```

The `RowExpr` operators: `eq`, `ne`, `gt`, `lt`, `gte`, `lte`, `in_subquery`, `not_in_subquery`. Rendering the comparison:

```text
(("created_at", "id") < ($1, $2))
```

The right operand is a tuple of values: every element is checked against the type of the corresponding row element, so a mismatched pair is refused at the call site. An index built over `(created_at, id)` is used by the engine natively — no client-side stitching of conditions happens.

## `DISTINCT` and `DISTINCT ON`

- `.distinct()` — deduplication; requires every `ORDER BY` expression to be present in the projection.
- `.distinct_on(col)` — a PostgreSQL specialty, gated by `SupportsDistinctOn`; the leading `ORDER BY` expressions must match the `DISTINCT ON` expressions left to right.
- Both rules compare the **assembled expressions**, not the types, so they are checked in `.build()`: `BuildError::DistinctOrderNotProjected { position }` and `BuildError::DistinctOnOrderMismatch { position }`.
- Deduplication removes the row-locking clauses: `FOR UPDATE` / `FOR SHARE` are unreachable on a deduplicated statement, and an assembled statement carrying both is rejected as `BuildError::DistinctOnWithLock` / `DistinctWithLock`.

## Order and set operations

A criterion recorded **before** `UNION`/`INTERSECT`/`EXCEPT` belongs to the left operand and renders inside its parentheses. The same call **after** the combination applies to the combined rows and requires the `order_by_output*` / `order_by_index*` forms (see [Set operations](set_ops.md)).

## Related chapters

- [The `SELECT` statement](select.md) — the clause anatomy and the evaluation order this chapter is one step of.
- [`GROUP BY` and `HAVING`](group_by.md) — ordering groups.
- [Window functions](window.md) — `ORDER BY` inside `OVER (...)`.
- [Predicates and `WHERE`](filtering.md)
