# Ordering and pagination

Ordering is described by the `.order_by*` family; pagination by `.limit(u32)` and `.offset(u32)`.

## Columns and column tuples

```rust
# extern crate rurqe;
# use rurqe::core::schema::At;
# use rurqe::core::ExprKind;
pub trait OrderBy<Tables, Positions = At<0>> {
    fn push_orderings(&self, out: &mut Vec<ExprKind>);
}
# fn main() {}
```

Implemented for a single column and for tuples of up to eight columns: a tuple unfolds into as many criteria as it has elements.

```rust,no_run
# extern crate rurqe;
# use rurqe::prelude::*;
# table! { User => "users" { UserId => "id": i64, UserName => "name": String, } }
# fn main() {
let built = select_from::<Postgres<Pg14>, User>()
    .select(select!(UserId, UserName))
    .where_(Expr::column::<UserId>().gt(10i64))
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

`order_by_expr*` accepts an `OrderExprTarget<Tables, P, D>`:

- `Expr<T, Scope, NonAggregate, ExprD>` — arithmetic, `CASE`, scalar functions;
- `WindowExpr<...>` — a whole window, with the `OVER (...)` inside the criterion.

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

> **Compiler Guard in Action.** The aggregate marker of the criterion is fixed as `NonAggregate`: a plain query yields one row per source row and sorts by a per-row value. An aggregate folds the rows of a group, so ordering by an aggregate lives only on `GroupedQuery::order_by_expr*` and is rejected at the call site in a plain query.

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

The right operand implements `RowValues<Row>`: every element is checked for compatibility with the type of the corresponding row element. An index built over `(created_at, id)` is used by the engine natively — no client-side stitching of conditions happens.

## `DISTINCT` and `DISTINCT ON`

- `.distinct()` — deduplication; requires every `ORDER BY` expression to be present in the projection.
- `.distinct_on(col)` — a PostgreSQL specialty, gated by `SupportsDistinctOn`; the leading `ORDER BY` expressions must match the `DISTINCT ON` expressions left to right.
- Both rules compare the **assembled expressions**, not the types, so they are checked in `.build()`: `BuildError::DistinctOrderNotProjected { position }` and `BuildError::DistinctOnOrderMismatch { position }`.
- Deduplication moves the query out of `NoLock`: `FOR UPDATE`/`FOR SHARE` are unreachable on such a statement, and an AST statement carrying both traits is rejected as `BuildError::DistinctOnWithLock` / `DistinctWithLock`.

## Order and set operations

A criterion recorded **before** `UNION`/`INTERSECT`/`EXCEPT` belongs to the left operand and renders inside its parentheses. The same call **after** the combination applies to the combined rows and requires the `order_by_output*` / `order_by_index*` forms (see [Set operations](set_ops.md)).

## Related chapters

- [`GROUP BY` and `HAVING`](group_by.md) — ordering groups.
- [Window functions](window.md) — `ORDER BY` inside `OVER (...)`.
- [Predicates and `WHERE`](filtering.md)
