# Common table expressions (`WITH`)

Three wrappers expose a finished statement as a relation of another one. All of them live in `rurqe-core/src/query/cte.rs`:

| Wrapper | SQL shape | Enters the outer statement through |
|---|---|---|
| `Cte<T, Row, D>` | `WITH "name" AS (SELECT ...)` | `.with(...)` |
| `DerivedTable<T, Row, D>` | `(SELECT ...) AS "name"` | `from_subquery(...)` / `*_join_subquery(...)` |
| `Lateral<T, Row, D>` | `LATERAL (SELECT ...)` | `cross_join_lateral(...)` / `left_join_lateral(...)` |

All three carry the row type `Row` and the dialect `D` of the wrapped statement, and render with the dialect of the statement embedding them, which the enclosing statement checks.

## `Cte`

```rust
pub fn cte<T: Table, D: Dialect, S: TableScope, Row>(
    query: SelectQuery<D, S, Row, HasProjection>,
) -> Cte<T, Row, D>;

pub fn cte_named<D: Dialect, S: TableScope, Row>(
    name: &'static str,
    query: SelectQuery<D, S, Row, HasProjection>,
) -> Cte<(), Row, D>;
```

`cte::<T, _, _, _>(query)` names the CTE after the table marker `T` — `T::NAME` — while `cte_named(name, query)` takes the name explicitly. A projected query also exposes `as_cte` and `as_named_cte` (`query/select/render.rs`), which wrap it into a `Cte` the same way.

The outer statement declares the CTE with `.with(...)` (`query/select/clauses.rs`) and renders:

```text
WITH "name" AS (SELECT ...) SELECT ... FROM "name" ...
```

`Cte<T, Row, D>` implements `Table` with `NAME = T::NAME`, so the outer statement addresses the CTE through a table marker exactly like a stored relation: it joins, projects, and nullability-checks the same way, and `Cte::name()` returns the SQL identifier the rendering pass uses.

## Derived tables

A `DerivedTable` exposes a projected query as an inline relation — no temporary table, no view, no second round trip. The marker `T` declares the shape of the projected rows, which keeps the columns of the relation checked against the scope of the statement reading them — including the `Option<T>` lifting an outer join applies:

```text
SELECT "user_summary"."order_count" FROM "users"
INNER JOIN (SELECT "user_id", COUNT(*) AS "order_count" FROM "orders" GROUP BY "user_id")
    AS "user_summary" ON ("user_summary"."user_id" = "users"."id")
```

The relation is registered under the name the rendering pass addresses it by: the alias the marker exposes it under, or its table name when the marker carries none. Recording the bare table name of an aliased marker would leave the subquery unmatched — the statement would read the stored table of that name instead of the rows of `query` — which is why `DerivedTable::new` resolves the reference through `T::table_ref()`.

Derived tables join through `.inner_join_subquery(derived, on)` / `.left_join_subquery(derived, on)` (`query/select/joins.rs`) and start a statement through `from_subquery` (`query/select/from.rs`). An outer-joined derived table lifts its columns to `Option<T>` exactly like a left-joined stored table — see [Joins](joins.md).

## `LATERAL`

`Lateral<T, Row, D>` wraps a correlated statement exposing its rows under the table marker `T`; `lateral(query)` and `query.as_lateral::<T>()` build one. It joins through `.cross_join_lateral(...)` / `.left_join_lateral(...)`, gated by `SupportsLateralJoin`; a `LEFT JOIN LATERAL` renders its condition as `ON true`, and the unmatched lateral rows lift the subquery projections into `Option<T>`. See [Joins](joins.md) for the join side and [Subqueries](subqueries.md) for the correlated-reference mechanism.

## Where this lives in the code

| Module | Contents |
|---|---|
| `rurqe-core/src/query/cte.rs` | `Cte`, `DerivedTable`, `Lateral`, `cte`, `cte_named`, `lateral` |
| `rurqe-core/src/query/select/clauses.rs` | `.with(...)` |
| `rurqe-core/src/query/select/from.rs` | `from_subquery` |
| `rurqe-core/src/query/select/render.rs` | `as_cte`, `as_named_cte`, `as_table`, `as_lateral` |
| `rurqe-core/src/query/select/joins.rs` | `*_join_subquery`, `*_join_lateral` |

## Related chapters

- [Subqueries](subqueries.md) — scalar, `EXISTS`, and `IN` subqueries.
- [Joins](joins.md)
- [`GROUP BY` and `HAVING`](group_by.md) — an aggregated statement as a derived table.
