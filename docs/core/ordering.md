# Ordering and pagination

Ordering is described by the `.order_by*` family; pagination by `.limit(u32)` and `.offset(u32)`.

Both are the last steps of the logical query processing order: the rows exist, the projection has named its output columns, and the ordering is applied to what they produced.

## Columns and criteria

```rust,no_run
{{#rustdoc_include ../examples/ordering.rs:order_by}}
```

```sql
SELECT "id" AS "user_id", "name" AS "user_name" FROM "users"
WHERE ("id" > $1) ORDER BY "id" ASC LIMIT 5 OFFSET 10
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

Criteria are recorded in the order they are added: `.order_by_desc(EventCreatedAt).order_by_desc(EventId)` orders by the creation time first and the identifier second.

## `NULLS FIRST` / `NULLS LAST`

Databases differ in where they place absent values, and an index or a keyset cursor needs the placement pinned. The nulls family makes it explicit:

```rust,no_run
{{#rustdoc_include ../examples/ordering.rs:nulls}}
```

`NULLS FIRST` / `NULLS LAST` is a dialect capability: the methods are unavailable on a dialect that does not support the placement.

## Ordering by an expression or a window

`.order_by_expr*` accepts two kinds of criterion:

- an expression — arithmetic, `CASE`, a scalar function;
- a whole window, with the `OVER (...)` inside the criterion.

```rust,no_run
{{#rustdoc_include ../examples/ordering.rs:window_order}}
```

A criterion of a plain query is a per-row value: the statement yields one row per source row, so ordering by an aggregate is refused there. An aggregate folds the rows of a group, so it orders the result only on a grouped statement.

## Keyset pagination with tuples

A tuple of columns compares against a tuple of values as a single value (SQL:1992):

```rust,no_run
{{#rustdoc_include ../examples/ordering.rs:keyset}}
```

```sql
(("created_at", "id") < ($1, $2))
```

The right operand is a tuple of values: every element is checked against the type of the corresponding row element, so a mismatched pair is refused at the call site. An index built over `(created_at, id)` is used by the engine natively — no client-side stitching of conditions happens.

The row operators: `eq`, `ne`, `gt`, `lt`, `gte`, `lte`, `in_subquery`, `not_in_subquery`.

## `DISTINCT` and `DISTINCT ON`

- `.distinct()` — deduplication; requires every `ORDER BY` expression to be present in the projection.
- `.distinct_on(col)` — a PostgreSQL specialty; the leading `ORDER BY` expressions must match the `DISTINCT ON` expressions left to right.
- Both rules compare the assembled expressions, not the types, so they are checked while the statement is built: `BuildError::DistinctOrderNotProjected { position }` and `BuildError::DistinctOnOrderMismatch { position }`.
- Deduplication removes the row-locking clauses: `FOR UPDATE` / `FOR SHARE` are unreachable on a deduplicated statement, and an assembled statement carrying both is rejected.

## Order and set operations

A criterion recorded before `UNION`/`INTERSECT`/`EXCEPT` belongs to the left operand and renders inside its parentheses. The same call after the combination applies to the combined rows and requires the `.order_by_output*` / `.order_by_index*` forms.
