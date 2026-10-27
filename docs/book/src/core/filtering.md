# Predicates and `WHERE`

Selection σ is the subset of tuples satisfying a predicate. A predicate in RURQE has the type `Expr<bool, Scope, Marker, D>`:

- `Scope` — the tables it reads;
- `Marker` — whether it aggregates rows (`Aggregate`) or is computed per row (`NonAggregate`);
- `D` — the dialect requirements inherited from its operands.

## Operators

| Method | SQL | Declared in |
|---|---|---|
| `.eq(v)`, `.ne(v)`, `.gt(v)`, `.lt(v)` | `=`, `<>`, `>`, `<` | `Expression` (columns) and `Expr` |
| `.between(low, high)` | `BETWEEN ... AND ...` | `Expression`, `Expr` |
| `.in_list(values)` / `.not_in_list(values)` | `IN (...)` / `NOT IN (...)` | `Expression`, `Expr` |
| `.is_null()` / `.is_not_null()` | `IS NULL` / `IS NOT NULL` | `Expr` |
| `.like(p)` / `.ilike(p)` | `LIKE` / `ILIKE` | `StringExpression` |
| `.in_subquery(q)` / `.not_in_subquery(q)` | `IN (SELECT ...)` | `Expr` |
| `.and(p)` / `.or(p)` | `AND` / `OR` | `Expr<bool, _>` |

Scalar comparisons cover only `eq`/`ne`/`gt`/`lt`; `gte`/`lte` exist on tuple comparisons (`RowExpr`, see [Ordering and pagination](ordering.md)).

```rust
use rurqe::prelude::*;

let built = select_from::<Postgres, User>()
    .select((UserName,))
    .where_(UserId.eq(1i64))
    .build()
    .expect("valid query");
```

```text
SELECT "name" FROM "users" WHERE ("id" = $1)
```

## A repeated `where_` folds with `AND`

```rust
use rurqe::prelude::*;

let built = update::<Postgres, User>()
    .set(UserName::set("dave"))
    .where_(UserId.gt(1i64))
    .where_(UserActive.eq(true))
    .build()
    .expect("valid query");
```

```text
UPDATE "users" SET "name" = $1 WHERE (("id" > $2) AND ("active" = $3))
```

The parameter order follows the order of appearance: first the `SET` values, then the predicates.

## Dynamic predicates

Queries are also assembled at runtime, without losing typestates:

| Method | What it does |
|---|---|
| `.where_opt(Option<Expr<bool, ..>>)` | records the predicate when it is present |
| `.when(cond, \|q\| q.where_(...))` | applies the closure when the condition is true |
| `.if_some(value, \|q, v\| q.where_(...))` | applies the closure on `Some`, passing the value |

```rust
use rurqe::postgres::prelude::*;

let filtered = select_from(Users)
    .select(cols!(UsersId, UsersName))
    .when(only_active, |q| q.where_(UsersActive.eq(true)))
    .if_some(search_term, |q, term| q.where_(UsersName.ilike(format!("%{term}%"))))
    .build()?;
```

The closure must return the **same** query type — it is bounded by `impl FnOnce(Self) -> Self`. A clause that changes the type (a projection, a join, a lock) does not fit inside `when`/`if_some`; separate forms exist for that (`where_if`, `where_opt_or_all` on mutations).

## An empty `IN` list

`x IN ()` is a syntax error in every dialect, so an empty membership test degenerates into a constant: `IN []` renders as `1 = 0`, and `NOT IN []` as `1 = 1`.

## What the compiler checks

1. **The predicate type.** `.where_(...)` accepts only `Expr<bool, ..>`.
2. **The scope.** Every table of the predicate must be present in `FROM`/`JOIN` (`ExprInScope`, `ScopePositions`). Comparing columns of two unjoined relations is rejected at the call site.
3. **Dialect requirements.** `PredD: DialectInScope<D>` — a predicate carrying a dialect operation does not fit into a query of another dialect.
4. **Aggregate folding.** The comparison marker is computed from both operands: `sum(x).gt(0.0)` stays an aggregate, and `sum(x).gt(col)` becomes per-row.

> **Compiler Guard in Action.** `WHERE ("total" > (SELECT AVG("total") FROM "orders"))` is produced from `Expr::column::<OrderTotal>().gt(average)`, where `average` is a single-column `SelectQuery`. A scalar subquery slots in without `.into()`; a multi-column one is rejected, because `IntoExpr<T>` is implemented only for a `(T,)` projection.

## What `.build()` adds

A predicate is an operator expression, so the operators nested inside it are checked pre-flight (the structural rules), the resolvability of qualifiers (`BuildError::UnresolvedQualifier`), and the placement of aggregates: an aggregate inside `WHERE` is a `BuildError::AggregateInRowClause` naming the clause `RowClause::Where`.

## Related chapters

- [Subqueries](subqueries.md) — `EXISTS`, `IN`, scalar subqueries.
- [Ordering and pagination](ordering.md) — tuple comparisons and keyset cursors.
- [DML](dml.md) — the same predicates in the `WHERE` of mutations, plus `where_if` / `where_opt_or_all`.
