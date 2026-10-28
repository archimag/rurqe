# Scopes: TableScope and ExprScope

Relational algebra requires every attribute to be bound to a relation and every relation to be present in the expression. In RURQE these are two independent trait systems: one describes the relations of the query, the other describes the scope an expression is allowed to reference.

## TableScope — the relations of the query

```rust,ignore
// Fragment of `rurqe-core/src/query/scope.rs`; `TableRef` is elided.
pub trait TableScope {
    fn push_table_names(out: &mut Vec<&'static str>);
    fn push_table_refs(out: &mut Vec<TableRef>) { /* by default, derived from the names */ }
}
```

- A single table implements `TableScope` trivially.
- Every join type implements it by appending its own relation:
  `Joined<Tables, T>`, `LeftJoined<Tables, T>`, `CrossJoined<Tables, T>`, `FullOuterJoined<Tables, T>`, `CrossLateralJoined<Tables, T>`, `LeftLateralJoined<Tables, T>`.
- `Compound` implements `TableScope` without adding a single relation: a compound query addresses only the output columns of its projection, not stored tables.

That is exactly why `TableInScope` has no implementation for `Compound`: a table reference inside a compound query is structurally unreachable.

## TableInScope — the position of a relation

```rust,ignore
// Fragment of `rurqe-core/src/schema/scope.rs`; `Table` and `At` are elided.
pub trait TableInScope<T: Table, P = At<0>> {}
```

- A table is trivially in its own scope: `impl<T: Table> TableInScope<T, At<0>> for T`.
- A joined scope adds the new relation at position `At<0>` and shifts every other one to `Next<P>`:

```rust,ignore
// Fragments of `rurqe-core/src/query/scope.rs`.
impl<Tables, T: Table> TableInScope<T, At<0>> for Joined<Tables, T> {}
impl<Tables, T: Table, Target: Table, P> TableInScope<Target, Next<P>> for Joined<Tables, T>
where Tables: TableInScope<Target, P> {}
```

The `At` / `Next` positions exist only for unambiguous inference: the same table cannot occur twice in a scope under one name, and the compiler needs a marker that distinguishes the occurrences.

## ExprScope and ExprInScope — the scope of an expression

An expression carries its scope as a phantom parameter: `Expr<T, Scope, Aggr, D>`. `Scope` is `()` for an expression without columns, a single table, a tuple of tables, or a joined scope type.

```rust,ignore
// Fragments of `rurqe-core/src/schema/scope.rs`; `At` is elided.
pub trait ExprScope {}
pub trait ExprInScope<Tables, Positions = At<0>> {}
```

The `ExprInScope` implementations are:

| `Self` | Condition |
|---|---|
| `()` | always — the expression references no columns |
| `Unscoped` | always — the scope was lifted deliberately and the enclosing query performs the check |
| `T: Table` | `Tables: TableInScope<T, P>` |
| `(T1, T2, ...)` | every `Tables: TableInScope<Ti, Pi>` |

`Unscoped` differs from `()` deliberately: it says "this expression references tables, but whoever nests it performs the check", which is what subqueries and `CAST`-like constructs need.

## ScopePositions — inferring positions

```rust,ignore
// Fragment of `rurqe-core/src/schema/scope.rs`; `At` is elided.
pub trait ScopePositions<P = At<0>> {}
```

The empty scope always stands at `At<0>`, a single table stands at its own position, and a tuple contributes one position per element. Without this trait, `P` in `ExprInScope<Tables, P>` would stay ambiguous and produce `E0282`.

## How this looks in use

```rust,no_run
# use rurqe::prelude::*;
# rurqe::table! {
#     User => "users" {
#         UserId => "id": i64,
#     }
# }
# rurqe::table! {
#     Order => "orders" {
#         OrderUserId => "user_id": i64,
#         OrderTotal => "total": f64,
#     }
# }
// Compiles: Order is in scope after the join.
let ok = select_from::<Postgres, User>()
    .inner_join(Order, Expr::column::<OrderUserId>().eq(Expr::column::<UserId>()))
    .select((UserId, OrderTotal));
# let _ = ok;
```

```rust,compile_fail
# use rurqe::prelude::*;
# rurqe::table! {
#     User => "users" {
#         UserId => "id": i64,
#     }
# }
# rurqe::table! {
#     Order => "orders" {
#         OrderUserId => "user_id": i64,
#         OrderTotal => "total": f64,
#     }
# }
// Does not compile: OrderTotal belongs to Order, which is absent from FROM.
let bad = select_from::<Postgres, User>().select((UserId, OrderTotal));
```

```text
error[E0277]: column `OrderTotal` cannot be projected from this query's scope
  = note: introduce the column's table with `from` or a join before projecting it
```

## Scope and outer joins

A scope answers not only "does the relation exist" but also "what is its nullability". That is the job of the second trait:

```rust,ignore
// Fragment of `rurqe-core/src/query/scope.rs`; `Table` and `At` are elided.
pub trait TableNullabilityInScope<T: Table, P = At<0>> {
    type Nullability;
}
```

- A base table, `Joined`, `CrossJoined`, and `CrossLateralJoined` → `NonNullableRel`.
- `LeftJoined` and `LeftLateralJoined` → `NullableRel` for their own relation, inheriting `NonNullableRel` for the inner ones.
- `FullOuterJoined` → `NullableRel` for **every** relation of the scope.

How this turns into `Option<T>` is described in [Nullability](nullability.md).

## Scope and DML

Mutations define their scope differently: `UPDATE ... FROM` and `DELETE ... USING` extend it through `ExtendScope`, while `RETURNING` is restricted by `TableSelection<T>` — only columns of the mutated table can be returned. The `using` / `from` methods themselves exist only on dialects implementing `SupportsDeleteUsing` / `SupportsUpdateFrom`.

## Summary

| Question | Trait |
|---|---|
| Which relations are present in the query? | `TableScope` |
| Is a specific relation present? | `TableInScope<T, P>` |
| Is the scope of an expression admissible? | `ExprScope` |
| Are all tables of the expression present in the query? | `ExprInScope<Tables, P>`, `ScopePositions<P>` |
| What is the nullability of a relation in this scope? | `TableNullabilityInScope<T, P>` |
| Which tables does a mutation add? | `ExtendScope<S>` |
