# Nested results: `nest_many` and `nest_one`

A one-to-many relation read naively costs either N+1 statements (one per parent) or a `JOIN` with a Cartesian product that the client folds into lists by hand. RURQE reads such a relation in a single statement: the engine aggregates the child rows of every parent into a JSON document, and the client decodes the document into the row type of the child projection — with no extra statements and no client-side stitching.

```rust
// crates/rurqe-core/src/nested.rs
pub fn nest_many<Q>(query: Q) -> NestMany<Q>;
pub fn nest_one<Q>(query: Q) -> NestOne<Q>;

pub struct NestMany<Q>(pub Q); // RustType = Vec<Row>
pub struct NestOne<Q>(pub Q);  // RustType = Option<Row>
```

## Example: users with their orders

```rust
use rurqe::postgres::prelude::*;

let report = select_from(Users)
    .select(select!(
        user_id: UserId,
        user_name: UserName,
        orders: nest_many(
            select_from(Orders)
                .select(select!(order_id: OrderId, total: OrderTotal))
                .order_by_asc(OrderCreatedAt)
        ),
    ))
    .fetch_all(&mut conn)?;

for row in report {
    println!("{} — {} orders", row.user_name, row.orders.len());
    for order in row.orders {
        println!("  #{} for {}", order.order_id, order.total);
    }
}
```

The outer projection is `select!`, not `dto!`: `nest_many(...)` is a computed projection, while `dto!` accepts only column markers and rejects a computed field with an expansion error (`use `select!` for computed projections`). For the same reason the nested projection is described with `select!` as well — its declaration becomes the element type of the collection.

## Row type and position

The projection implements `SelectTarget` (the value type) and `SelectTargetInScope` (the value type together with its scope):

```rust
impl<D, S, Row> SelectTarget for NestMany<SelectQuery<D, S, Row, HasProjection>>
where
    D: Dialect,
    S: TableScope,
    Row: MultisetRow,
{
    type RustType = Vec<Row>;

    fn to_expr_kind(&self) -> ExprKind {
        nested_kind(&self.0, false)
    }
}

impl<Tables, InnerD, QueryD, S, Row> SelectTargetInScope<Tables, At<0>, QueryD>
    for NestMany<SelectQuery<InnerD, S, Row, HasProjection>>
where
    InnerD: Dialect + DialectInScope<QueryD>,
    QueryD: SupportsJsonAggregation,
    S: TableScope,
    Row: MultisetRow,
{
    type Output = Vec<Row>;
}
```

The position is fixed at `At<0>`: a nested projection reads from its own statement and references no table of the enclosing scope, so there is nothing to infer the position from — a fixed position keeps the marker determined instead of leaving inference to pick one of the positions a scope may offer.

## Correlation with the outer query

A nested statement is a `SelectQuery` of its own, and its typed scope contains only its own `FROM`. A column of the outer query enters a predicate through `without_scope()`, which replaces the tables of the expression with the `Unscoped` marker: such an expression is accepted in any scope because its tables are checked by the enclosing statement.

```rust
orders: nest_many(
    select_from(Orders)
        .select(select!(order_id: OrderId, total: OrderTotal))
        .where_(OrderUserId.eq(UserId.without_scope()))
)
```

## `nest_one`: an absent child as `None`

`nest_one` differs from `nest_many` only in that the aggregation stops at the first row: a missing child yields `None` rather than an empty vector, so "no value" stays explicit and is not mixed up with "present but empty".

## Row type contract

The element type declares the keys its fields are read from for one row of the document:

```rust
pub trait MultisetRow: SqlArrayElement + 'static {
    /// The JSON keys of the projected fields, in projection order.
    const FIELDS: &'static [&'static str];
}
```

`select!` synthesizes the record together with the `FromRow`, `ToSqlVal`, `FromSqlValue`, `SqlArrayElement`, and `MultisetRow` implementations; `FIELDS` holds the field names in projection order, and the same names become the keys of the JSON document (the field `r#type` is written as `type`).

`dto!` declares the same set of contracts (`MultisetRow`, `ToSqlVal`, `FromSqlValue`, `SqlArrayElement`) with the same keys in declaration order, but emits no projection: a DTO is a decoding type and a nested-row type, not a projection list, so `select!` projects a row while the DTO reads it.

## Compiler Guard in Action

- **No native JSON aggregation — no projection.** `QueryD: SupportsJsonAggregation` in `SelectTargetInScope` means `nest_many` does not compile for `Sqlite<Sqlite3_35>`: the rejection arrives at the type level, not from the engine.
- **Projection without rows.** A nested statement must be in the `HasProjection` state; `nest_many(select_from(Orders))` without `.select(...)` cannot be nested.
- **The row type must declare the keys.** `Row: MultisetRow` ties the document to the field declaration, so an arbitrary element type does not qualify.

## Related chapters

- [MULTISET: how nesting turns into SQL](multiset.md)
- [Engines, capability gates, and decoding](multiset_engines.md)
- [NF² and MULTISET: nested relations](../theory/nf2_multiset.md)
