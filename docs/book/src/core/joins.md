# Joins

A join adds a relation to the `FROM` clause and reinterprets the query under the scope type describing the joined relations. Every builder records a `JoinAst` and moves the query into the new scope through `with_scope`, so the relational effect of a join is expressed by the return type instead of by the rendered clause.

## The join family

| Method | SQL | Gated by |
|---|---|---|
| `.inner_join(table, on)` | `INNER JOIN "table" ON (...)` | — |
| `.left_join(table, on)` | `LEFT JOIN "table" ON (...)` | — |
| `.cross_join(table)` | `CROSS JOIN "table"` | — |
| `.full_outer_join(table, on)` | `FULL OUTER JOIN "table" ON (...)` | `SupportsFullOuterJoin` |
| `.inner_join_subquery(derived, on)` | `INNER JOIN (SELECT ...) AS "name" ON (...)` | — |
| `.left_join_subquery(derived, on)` | `LEFT JOIN (SELECT ...) AS "name" ON (...)` | — |
| `.cross_join_lateral(lateral)` | `CROSS JOIN LATERAL (SELECT ...)` | `SupportsLateralJoin` |
| `.left_join_lateral(lateral)` | `LEFT JOIN LATERAL (SELECT ...) ON true` | `SupportsLateralJoin` |

The join builders take the table marker as a value and read the joined type `T` from it, so the compiler infers `T`, the joined scope, and the position marker at the call site: no turbofish is written and no runtime cost is paid, because the marker is zero-sized.

```rust,no_run
# extern crate rurqe;
# use rurqe::postgres::prelude::*;
# table! { User => "users" { UserId => "id": i64, UserName => "name": String, } }
# table! { Order => "orders" { OrderId => "id": i64, OrderTotal => "total": f64, OrderUserId => "user_id": i64, } }
# fn main() {
{{#rustdoc_include ../../../../examples/joins.rs:inner}}
# }
```

```text
SELECT "users"."id", "users"."name", "orders"."total"
FROM "users" INNER JOIN "orders" ON ("orders"."user_id" = "users"."id")
```

## The `ON` predicate and the scope

The `on` predicate may read the tables already in scope and the joined table; a condition referencing a table absent from `FROM`/`JOIN` is rejected at the call site. Every join appends its scope type to the chain: `Joined<Tables, T>`, `LeftJoined<Tables, T>`, `CrossJoined<Tables, T>`, `FullOuterJoined<Tables, T>`, `CrossLateralJoined<Tables, T>`, `LeftLateralJoined<Tables, T>`. A `CROSS JOIN` takes no `ON`: it pairs every row unconditionally.

A column marker is a complete operand on its own, on either side of the comparison: `OrderUserId.eq(UserId)` compares two columns without any wrapper, exactly as `OrderTotal.gt(0.0f64)` compares a column with a bound value. Both operands declare the tables they reference, so the predicate's scope carries every relation it reads, and the join builder checks the `ON` clause against the relations the statement declares — a column of a relation the query does not introduce is still refused at the call site. `Expr::column::<C>()` stays the constructor for the positions that need an expression value standing on its own, and correlation still goes through `.without_scope()`, because the marker always claims its table.

## Nullability of outer joins

`LEFT JOIN`, `FULL OUTER JOIN`, and `LEFT JOIN LATERAL` may leave the joined relation unmatched, so its columns reach the projected row as `Option<T>`:

```rust,no_run
# extern crate rurqe;
# use rurqe::postgres::prelude::*;
# table! { User => "users" { UserId => "id": i64, UserName => "name": String, } }
# table! { Order => "orders" { OrderId => "id": i64, OrderTotal => "total": f64, OrderUserId => "user_id": i64, } }
# fn main() {
{{#rustdoc_include ../../../../examples/joins.rs:outer}}
# }
```

The lifting is automatic: the outer-join scope types mark the joined relation nullable, and `SelectionFor::Row` computes `Option<T>` for its columns — `order_total` arrives as `Option<f64>`. `FULL OUTER JOIN` lifts both sides — the columns of the left and the right relation all arrive as `Option<T>`. Reading an outer-join column as a plain `T` fails to compile, so a missing row can never be decoded into a non-nullable field (see [Nullability](../theory/nullability.md)).

## Unaliased duplicate relations

Joining a table the query already addresses renders its name twice in the `FROM` clause, which the engine rejects as `table name specified more than once` unless the second relation is aliased. The compiler refuses the statement even earlier: the second occurrence would need its own relation scope and position, which the bare marker does not provide.

```rust,compile_fail
# extern crate rurqe;
# use rurqe::postgres::prelude::*;
# table! { User => "users" { UserId => "id": i64, UserManagerId => "manager_id": i64, } }
# fn main() {
// Rejected by the compiler: `users` is already the relation the statement
// selects from, so the bare marker cannot stand for a second occurrence.
let _ = select_from(User)
    .inner_join(User, UserManagerId.eq(Expr::column::<UserId>()))
    .select(cols!(UserId));
# }
```

The fix is to alias the second occurrence through the alias markers — see [Aliases](aliasing.md).

## Joining derived tables

`.inner_join_subquery(derived, on)` and `.left_join_subquery(derived, on)` join a `DerivedTable` — a finished statement exposed as a relation. The wrapped query renders inline as `(SELECT ...) AS "name"`, so the joined relation behaves like a stored one while its rows come from the subquery; a left-joined derived table lifts its columns to `Option<T>` exactly like a left-joined stored table. Building derived tables is covered in [Common table expressions](cte.md).

## `LATERAL` joins

`LATERAL` joins are gated by `SupportsLateralJoin` — implemented for PostgreSQL, absent for SQLite, so the methods do not exist there at compile time:

```rust,no_run
# extern crate rurqe;
# use rurqe::postgres::prelude::*;
# table! { User => "users" { UserId => "id": i64, } }
# table! { Order => "orders" { OrderTotal => "total": f64, OrderUserId => "user_id": i64, OrderCreatedAt => "created_at": i64, } }
table! {
    /// The `latest_order` lateral relation marker.
    LatestOrder => "latest_order" {
        LatestOrderTotal => "total": f64,
    }
}
# fn main() -> Result<(), Box<dyn std::error::Error>> {
let report = select_from(User)
    .left_join_lateral(lateral::<LatestOrder, _, _, _>(
        select_from(Order)
            .select((OrderTotal,))
            .where_(OrderUserId.eq(Expr::column::<UserId>().without_scope()))
            .order_by_desc(OrderCreatedAt)
            .limit(1),
    ))
    .select(cols!(UserId, LatestOrderTotal))
    .build()?;
// Row: (i64, Option<f64>) — the lateral projection lifts through the left join.
# let _ = report;
# Ok(())
# }
```

A `LEFT JOIN LATERAL` renders its condition as `ON true` and lifts the lateral projections into `Option<T>`; a correlated reference is written as an unscoped expression and resolved against the enclosing statement's relations (see [Subqueries](subqueries.md)).

## Related chapters

- [Common table expressions](cte.md) — derived tables and `LATERAL`.
- [Aliases](aliasing.md) — the remedy for duplicate relations.
- [Nullability](../theory/nullability.md) — the outer-join lifting rules.
