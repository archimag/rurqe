# Subqueries

A subquery is a whole statement nested inside another one. RURQE distinguishes three roles a nested `SELECT` can play — a scalar value, an `EXISTS` predicate, and an `IN` list — and a fourth, relational one, where the subquery becomes a relation of the outer statement (see [Common table expressions](cte.md)).

## Scalar subqueries

A `SelectQuery` projecting exactly one column implements `IntoExpr<T>` for the projected type, so it slots directly into any operator position:

```rust
use rurqe::postgres::prelude::*;

let above_average = select_from(Products)
    .select(cols!(ProductId, ProductName, ProductPrice))
    .where_(
        ProductPrice.gt(select_from(Products).select(avg(ProductPrice))),
    )
    .build()?;
```

```text
SELECT "id", "name", "price" FROM "products" WHERE ("price" > (SELECT AVG("price") FROM "products"))
```

The same conversion exists through `From`, so `.into()` works where the target type is unambiguous, and `Expr::subquery(query)` names it explicitly where it is not — inside a `CASE` branch or a tuple comparison, for example.

A multi-column projection does not implement `IntoExpr<T>` and is rejected by the compiler: a scalar position accepts exactly a `(T,)` projection.

## `EXISTS`

An `EXISTS` predicate is built from a projected query:

```rust
use rurqe::postgres::prelude::*;

let buyers = select_from(Users)
    .select(cols!(UserId, UserName))
    .where_(Expr::exists(
        select_from(Orders)
            .select((OrderUserId,))
            .where_(OrderUserId.eq(Expr::column::<UserId>().without_scope())),
    ))
    .build()?;
```

`Expr::exists` and `Expr::not_exists` live on `Expr<bool, ()>`; an unlocked projected query also exposes `.exists()` / `.not_exists()` / `.into_exists()` / `.into_not_exists()` directly (`query/select/render.rs`).

The reference `Expr::column::<UserId>().without_scope()` is a **correlated** one: `.without_scope()` substitutes the `Unscoped` marker, the reference keeps its qualifier (`"users"."id"`), and the pre-flight qualifier validation resolves it against the relations of the nested statement **plus the relations of every enclosing statement** — the namespace a correlated reference reaches. A qualifier naming no relation on either side is rejected as `BuildError::UnresolvedQualifier` before any SQL reaches the engine.

## `IN (SELECT ...)`

`.in_subquery(q)` / `.not_in_subquery(q)` accept a nested statement whose projection matches the outer expression:

```rust
use rurqe::postgres::prelude::*;

let with_orders = select_from(Users)
    .select(cols!(UserId, UserName))
    .where_(UserId.in_subquery(
        select_from(Orders).select((OrderUserId,)),
    ))
    .build()?;
```

```text
SELECT "id", "name" FROM "users" WHERE "id" IN (SELECT "user_id" FROM "orders")
```

`InSubquery<Row>` ties the expected row type to the nested projection: an `Expr<T>` / `Column<RustType = T>` accepts only a `(T,)` projection, and a row tuple `(a, b)` — a `RowExpr` — accepts only `(T1, T2)`. An arity- or type-mismatched projection is rejected by `rustc` at the call site.

## Deferred rendering and parameter numbering

A nested statement renders only when the outer statement does, and its parameters join the same list, so the numbering stays unified across scopes:

```text
WHERE ("total" > $1 AND EXISTS (SELECT ... WHERE "user_id" = $2))
```

## Subqueries as relations

A projected query can also become a relation of the outer statement: `as_table` / `from_subquery` expose it as a derived table, and `into_subquery` converts it into a `Subquery` node. That half of the story is told in [Common table expressions](cte.md).

## Where this lives in the code

| Module | Contents |
|---|---|
| `rurqe-core/src/expr/ops/subquery.rs` | `AnySubquery`, `InSubquery` |
| `rurqe-core/src/expr/ops/conversion.rs` | `IntoExpr<T>` for a single-column `SelectQuery` |
| `rurqe-core/src/expr/builder/predicates.rs` | `in_subquery` / `not_in_subquery` |
| `rurqe-core/src/expr/row.rs` | `RowExpr::in_subquery` / `not_in_subquery` |
| `rurqe-core/src/expr/ast.rs` | `Expr::exists` / `Expr::not_exists` |
| `rurqe-core/src/query/select/render.rs` | `.exists()`, `.into_subquery()` |

## Related chapters

- [Predicates and `WHERE`](filtering.md) — where the predicates land.
- [Common table expressions](cte.md) — CTEs, derived tables, and `LATERAL`.
- [Set operations](set_ops.md)
