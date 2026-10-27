# Joins

A join adds a relation to the `FROM` clause and reinterprets the query under the scope type describing the joined relations. Every builder records a `JoinAst` and moves the query into the new scope through `with_scope`, so the relational effect of a join is expressed by the return type instead of by the rendered clause.

## The join family

| Method | SQL | Gated by |
|---|---|---|
| `.inner_join_table(table, on)` | `INNER JOIN "table" ON (...)` | — |
| `.left_join_table(table, on)` | `LEFT JOIN "table" ON (...)` | — |
| `.cross_join_table(table)` | `CROSS JOIN "table"` | — |
| `.full_outer_join_table(table, on)` | `FULL OUTER JOIN "table" ON (...)` | `SupportsFullOuterJoin` |
| `.inner_join_subquery(derived, on)` | `INNER JOIN (SELECT ...) AS "name" ON (...)` | — |
| `.left_join_subquery(derived, on)` | `LEFT JOIN (SELECT ...) AS "name" ON (...)` | — |
| `.cross_join_lateral(lateral)` | `CROSS JOIN LATERAL (SELECT ...)` | `SupportsLateralJoin` |
| `.left_join_lateral(lateral)` | `LEFT JOIN LATERAL (SELECT ...) ON true` | `SupportsLateralJoin` |

The older turbofish forms `inner_join` / `left_join` / `cross_join` / `full_outer_join` still exist as `#[doc(hidden)]` aliases; the `*_table` entry points are preferred, because passing the table value lets the compiler infer `T`, the joined scope, and the position marker without a turbofish.

```rust
use rurqe::postgres::prelude::*;

let built = select_from(Users)
    .inner_join_table(
        Orders,
        OrderUserId.eq(UserId),
    )
    .select(cols!(UserId, OrderTotal))
    .build()?;
```

```text
SELECT "users"."id", "orders"."total"
FROM "users" INNER JOIN "orders" ON ("orders"."user_id" = "users"."id")
```

## The `ON` predicate and the scope

```rust
pub fn inner_join_table<T: Table, Scope, P>(
    self,
    _table: T,
    on: Expr<bool, Scope>,
) -> SelectQuery<D, Joined<Tables, T>, Output, Projected, Lock>
where
    Tables: TableScope,
    Scope: ExprInScope<Joined<Tables, T>, P> + ScopePositions<P>,
```

The `on` predicate may read the tables already in scope and the joined table; a condition referencing a table absent from `FROM`/`JOIN` is rejected at the call site. Every join appends its scope type to the chain: `Joined<Tables, T>`, `LeftJoined<Tables, T>`, `CrossJoined<Tables, T>`, `FullOuterJoined<Tables, T>`, `CrossLateralJoined<Tables, T>`, `LeftLateralJoined<Tables, T>`. A `CROSS JOIN` takes no `ON`: it pairs every row unconditionally.

## Nullability of outer joins

`LEFT JOIN`, `FULL OUTER JOIN`, and `LEFT JOIN LATERAL` may leave the joined relation unmatched, so its columns reach the projected row as `Option<T>`:

```rust
use rurqe::postgres::prelude::*;

let built = select_from(Users)
    .left_join_table(Orders, OrderUserId.eq(UserId))
    .select(cols!(UserId, OrderTotal))
    .build()?;
// Row: (i64, Option<f64>) — `orders.total` lifts automatically.
```

The lifting is automatic: the outer-join scope types mark the joined relation nullable, and `SelectionFor::Row` computes `Option<T>` for its columns. `FULL OUTER JOIN` lifts both sides — the columns of the left and the right relation all arrive as `Option<T>`. Reading an outer-join column as a plain `T` fails to compile, so a missing row can never be decoded into a non-nullable field (see [Nullability](../theory/nullability.md)).

## Unaliased duplicate relations

Joining a table the query already addresses renders its name twice in the `FROM` clause, which the engine rejects as `table name specified more than once` unless the second relation is aliased. The same effective name — the alias a relation is referenced under, or its table name when it carries none — collides exactly like two uses of one alias, so `.build()` rejects the statement pre-flight with `BuildError::DuplicateRelation { name }`, pointing at `table_alias!` as the remedy:

```rust
use rurqe::postgres::prelude::*;

// Rejected while building: two relations share the effective name "users".
let error = select_from(User)
    .inner_join_table(User, UserManagerId.eq(UserId))
    .select((UserId,))
    .build()
    .unwrap_err();
```

The fix is to alias the second occurrence through the alias markers — see [Aliases](aliasing.md).

## Joining derived tables

`.inner_join_subquery(derived, on)` and `.left_join_subquery(derived, on)` join a `DerivedTable` — a finished statement exposed as a relation. The wrapped query renders inline as `(SELECT ...) AS "name"`, so the joined relation behaves like a stored one while its rows come from the subquery; a left-joined derived table lifts its columns to `Option<T>` exactly like a left-joined stored table. Building derived tables is covered in [Common table expressions](cte.md).

## `LATERAL` joins

`LATERAL` joins are gated by `SupportsLateralJoin` — implemented for PostgreSQL, absent for SQLite, so the methods do not exist there at compile time:

```rust
use rurqe::postgres::prelude::*;

table! {
    /// The `latest_order` lateral relation marker.
    LatestOrder => "latest_order" {
        LatestOrderTotal => "total": f64,
    }
}

let report = select_from(Users)
    .left_join_lateral(lateral::<LatestOrder, _, _, _>(
        select_from(Orders)
            .select((OrderTotal,))
            .where_(OrderUserId.eq(Expr::column::<UserId>().without_scope()))
            .order_by_desc(OrderCreatedAt)
            .limit(1),
    ))
    .select(cols!(UserId, LatestOrderTotal))
    .build()?;
// Row: (i64, Option<f64>) — the lateral projection lifts through the left join.
```

A `LEFT JOIN LATERAL` renders its condition as `ON true` and lifts the lateral projections into `Option<T>`; a correlated reference is written as an unscoped expression and resolved against the enclosing statement's relations (see [Subqueries](subqueries.md)).

## Related chapters

- [Common table expressions](cte.md) — derived tables and `LATERAL`.
- [Aliases](aliasing.md) — the remedy for duplicate relations.
- [Nullability](../theory/nullability.md) — the outer-join lifting rules.
