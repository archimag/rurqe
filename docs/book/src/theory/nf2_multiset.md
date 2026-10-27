# NF² and MULTISET: nested relations

The classical relational model requires first normal form: attribute values are atomic. The NF² model (Non-First-Normal-Form) allows an attribute to be a nested relation. RURQE implements this through a single SQL aggregation rather than client-side stitching.

## The problem

A naive one-to-many relationship produces either N+1 queries (one per parent) or a Cartesian product in a `JOIN` from which the client assembles the nested lists by hand. Both variants are a source of bugs and extra traffic.

## The solution: a multiset projection

```rust
pub fn nest_many<Q>(query: Q) -> NestMany<Q>;
pub fn nest_one<Q>(query: Q) -> NestOne<Q>;

pub struct NestMany<Q>(pub Q);  // RustType = Vec<Row>
pub struct NestOne<Q>(pub Q);   // RustType = Option<Row>
```

The projection looks like an ordinary element of the projection list:

```rust
let users_with_orders = select_from(Users)
    .select(dto!(UserWithOrders {
        user_id: UserId,
        user_name: UserName,
        orders: nest_many(
            select_from(Orders)
                .select(dto!(OrderSummary {
                    order_id: OrderId,
                    total: OrderTotal,
                }))
                .where_(OrderUserId.eq(UserId))
        ),
    }));
```

Here `where_(OrderUserId.eq(UserId))` is a correlation with the outer query: the nested statement sees `UserId` because `ExprInScope` checks it against the enclosing query, while `Unscoped` inside the subquery means "whoever nests it performs the check".

## How it renders

The projection becomes an AST node:

```rust
ExprKind::Multiset {
    query: Subquery::new(query.to_ast()),
    fields: Row::FIELDS.to_vec(),
    single,
}
```

PostgreSQL:

```sql
SELECT "id",
       COALESCE((SELECT jsonb_agg(to_jsonb("__rurqe_sub"))
                 FROM (SELECT "sku" AS "sku", "qty" AS "qty" FROM "lines")
                 AS "__rurqe_sub"), '[]'::jsonb)
FROM "orders"
```

A dialect without native row aggregation builds every object through `json_object` and the array through `json_group_array`. The client decodes the same JSON document regardless of the engine.

The SQLite JSON functions are available starting from the version whose build always bundles them: nested projections therefore use the `sqlite3_38` prelude (`Sqlite<Sqlite3_38>`), and the requirement is expressed by the `SupportsJsonAggregation` trait.

## Compiler requirements

```rust
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

Three invariants:

1. **One dialect.** The nested statement belongs to the same dialect as the enclosing query (`DialectInScope<QueryD>`).
2. **Native aggregation.** The dialect must implement `SupportsJsonAggregation`; otherwise the projection simply does not compile instead of failing inside the engine.
3. **Row type.** `Row: MultisetRow` declares the keys of the document.

The `At<0>` position is fixed: a nested projection reads from its own statement and references no tables of the outer scope, so there is no position to infer.

## `MultisetRow` and the document

```rust
pub trait MultisetRow: SqlArrayElement + 'static {
    /// The JSON keys of the projected fields, in projection order.
    const FIELDS: &'static [&'static str];
}
```

The `FromSqlValue` implementations receive the JSON object of a single row — as `SqlVal::Json` from PostgreSQL or as JSON text from SQLite — and turn it into an object through `json_object`. The reverse direction (for example when passing a document as a parameter) is provided by `multiset_object` / `sql_val_to_json`.

The representation conventions:

| Value type | In JSON |
|---|---|
| `i32`, `i64` | a number |
| `f64` | a number; `NaN` and infinities become `null` |
| `bool` | a boolean |
| `String` | a string |
| `Vec<u8>` | the canonical hex text that `FromSqlValue for Vec<u8>` reads back |
| `Decimal` (the `decimal` feature) | a number when every digit is preserved, otherwise text |
| `Uuid`, `NaiveDateTime`, `DateTime<Utc>`, `NaiveDate` | the `Display` text that `FromSqlValue` parses back |
| nested JSON | as is |

Because of this, a field of a nested row is decoded by the same `FromSqlValue` contract as a column of the same type.

## Why MULTISET and not a set

Codd's relational algebra operates on sets: duplicates are impossible. SQL operates on multisets: `SELECT` does not remove duplicates without `DISTINCT`. RURQE follows SQL rather than the textbook:

- a set of rows is a multiset, ordered by `ORDER BY`;
- `Vec<Row>` in a nested projection is exactly that multiset, in the order the nested query specifies;
- `DISTINCT` / `DISTINCT ON` are separate operations that change the typestate (after them row locking is unavailable, recorded both in the types and in `BuildError::DistinctWithLock` / `DistinctOnWithLock`).

`nest_one` differs from `nest_many` only in that the aggregation stops at the first row: a missing child yields `None` rather than an empty vector, so "no value" stays explicit.

## What this gives

| Before | After |
|---|---|
| N+1 queries | one query |
| a Cartesian product with client-side stitching | native aggregation |
| manual JSON parsing | `Vec<ChildDto>` / `Option<ChildDto>` with decoding |
| a "no such JSON function" runtime error | a compile error without `SupportsJsonAggregation` |

## Related chapters

- [Codd → Rust](codd_to_rust.md) — where NF² sits in the operator table.
- [Scopes](scopes.md) — why a correlation inside a subquery is admissible while a reference to an unrelated table is not.
