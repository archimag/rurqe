# MULTISET: how nesting turns into SQL

A nested projection is not stitched together on the client: it becomes an AST node that renders as a single JSON aggregation.

## AST node

```rust
// crates/rurqe-core/src/nested.rs
fn nested_kind<D, S, Row>(query: &SelectQuery<D, S, Row, HasProjection>, single: bool) -> ExprKind
where
    D: Dialect,
    S: TableScope,
    Row: MultisetRow,
{
    ExprKind::Multiset {
        query: Subquery::new(query.to_ast()),
        fields: Row::FIELDS.to_vec(),
        single,
    }
}
```

The node carries three things: the nested statement, the document keys, and the "at most one row" flag.

## Document keys

The renderer (`crates/rurqe-core/src/expr/render/nested.rs`) assigns the nested statement `projection_aliases = Some(fields)`: every projected expression gets the name of the field it feeds. The row document therefore carries exactly the keys `MultisetRow::FIELDS` declares, while the clauses of the nested query — filter, ordering, limit — stay in force and decide the contents of the collection.

## Two rendering forms

The form is chosen by the `Dialect::SUPPORTS_JSONB_ROW_AGG` constant. Both forms produce the same JSON document, so the client decodes one representation regardless of the engine that produced it.

### PostgreSQL: native row aggregation

With `SUPPORTS_JSONB_ROW_AGG = true`, a collection renders as `jsonb_agg(to_jsonb(...))`, and an absence of rows is replaced with an empty array:

```sql
COALESCE((SELECT jsonb_agg(to_jsonb("__rurqe_sub"))
          FROM (SELECT "order_id" AS "order_id", "total" AS "total" FROM "orders")
               AS "__rurqe_sub"), '[]'::jsonb)
```

### SQLite: object per field

A dialect without native row aggregation assembles every object with `json_object` field by field and collects the objects with `json_group_array`; the keys are written as string literals and the values are read through the `__rurqe_sub` alias:

```sql
COALESCE((SELECT json_group_array(json_object('order_id', "__rurqe_sub"."order_id",
                                               'total', "__rurqe_sub"."total"))
          FROM (SELECT "order_id" AS "order_id", "total" AS "total" FROM "orders")
               AS "__rurqe_sub"), '[]')
```

## `nest_one`: aggregate without `COALESCE` and a limit of 1

For a single row the form differs in two details: the aggregate is not wrapped in `COALESCE`, and the limit of the nested query becomes `1` — either the limit of the compound statement, when a `UNION` is present:

```rust
if single {
    if inner.set_operations.is_empty() {
        inner.limit = Some(1);
    } else {
        inner.compound_limit = Some(1);
    }
}
```

PostgreSQL: `(SELECT to_jsonb("__rurqe_sub") FROM (...) AS "__rurqe_sub")`. SQLite: `(SELECT json_object(...) FROM (...) AS "__rurqe_sub")`. The ordering and pagination of the nested query decide which row reaches `Option<Row>`, while the absence of a row yields `NULL` — that is, `None` on decoding.

## Raw fragments and placeholders

`render_multiset` returns `Result<(), RawSqlError>`: a `sql!` expression inside a nested query may declare a parameter count that disagrees with the actual fragments, and the disagreement is reported as `RawSqlError` instead of sending SQL with a lost or shifted placeholder.

## A multiset, not a set

- `Vec<Row>` keeps the order set by the `ORDER BY` of the nested query and keeps duplicates: SQL without `DISTINCT` does not remove them.
- `nest_one` returns `Option<Row>`: an absent child is `None`, not an empty collection.
- `DISTINCT` and `DISTINCT ON` are a separate operation over a statement with its own typestate, not a property of a nested projection.

## Related chapters

- [Nested results: nest_many and nest_one](nested.md)
- [Engines, capability gates, and decoding](multiset_engines.md)
- [NF² and MULTISET: nested relations](../theory/nf2_multiset.md)
