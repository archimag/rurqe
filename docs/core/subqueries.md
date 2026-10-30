# Subqueries

A subquery is a whole statement nested inside another one. RURQE distinguishes three roles a nested `SELECT` can play — a scalar value, an `EXISTS` predicate, and an `IN` list — and a fourth, relational one, where the subquery becomes a relation of the outer statement.

## Scalar subqueries

A `SELECT` projecting exactly one column is also a scalar operand, so it slots directly into any operator position:

```rust,no_run
{{#rustdoc_include ../examples/subqueries.rs:scalar}}
```

```sql
SELECT "id", "name", "price" FROM "products"
WHERE ("price" > (SELECT AVG("price") FROM "products"))
```

The same conversion exists through `From`, so `.into()` works where the target type is unambiguous, and `Expr::subquery(query)` names it explicitly where it is not — inside a `CASE` branch or a tuple comparison, for example.

A multi-column projection is not a scalar operand and is rejected by the compiler: a scalar position accepts exactly a one-column projection of the compared type.

## `EXISTS`

An `EXISTS` predicate is built from a projected query:

```rust,no_run
{{#rustdoc_include ../examples/subqueries.rs:exists}}
```

`Expr::exists` and `Expr::not_exists` take the nested statement; an unlocked projected query also exposes `.exists()` / `.not_exists()` directly.

The reference `Expr::column::<UserId>().without_scope()` is a correlated one: `.without_scope()` lifts the reference out of the nested statement's own relations, and the pre-flight validation resolves it against the relations of the nested statement plus the relations of every enclosing statement — the namespace a correlated reference reaches. A qualifier naming no relation on either side is rejected before any SQL reaches the engine.

## `IN (SELECT ...)`

`.in_subquery(q)` / `.not_in_subquery(q)` accept a nested statement whose projection matches the outer expression:

```rust,no_run
{{#rustdoc_include ../examples/subqueries.rs:in_subquery}}
```

```sql
SELECT "id", "name" FROM "users" WHERE "id" IN (SELECT "user_id" FROM "orders")
```

The expected row type is tied to the nested projection: an expression of type `T` accepts only a `(T,)` projection, and a row tuple `(a, b)` accepts only `(T1, T2)`. An arity- or type-mismatched projection is rejected at the call site.

## Deferred rendering and parameter numbering

A nested statement renders only when the outer statement does, and its parameters join the same list, so the numbering stays unified across scopes:

```sql
WHERE ("total" > $1 AND EXISTS (SELECT ... WHERE "user_id" = $2))
```

## Subqueries as relations

A projected query can also become a relation of the outer statement: `as_table` exposes it as a derived table that `from` and the joins accept, and `into_subquery` converts it into a subquery node. The `WITH` chapters cover the same mechanism under its named form.
