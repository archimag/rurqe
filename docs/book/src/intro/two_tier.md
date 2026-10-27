# Two-Tier SQL Correctness

The guarantee "valid SQL or a compile error" cannot be expressed entirely inside the open type system of stable Rust. RURQE splits it into two levels, each with its own mechanism.

## Tier 1 — the type system (stable Rust)

Everything that can be stated as a positive assertion ("the relation exists") is checked by the compiler at the call site.

| Invariant class | Traits | What is rejected |
|---|---|---|
| Relational scope | `TableInScope`, `ExprInScope`, `ExprScope`, `ScopePositions`, `ColumnInScope`, `SelectTargetInScope`, `SelectionFor`, `SelectionInScope`, `GroupByInScope`, `TableSelection`, `ColumnBelongsTo` | projecting, filtering, grouping, ordering, joining, assigning, or returning a column of a relation absent from `FROM` / `JOIN`; comparing columns of unrelated relations |
| Outer-join nullability | `TableNullabilityInScope`, `MakeNullable`, `ApplyNullability` | reading a `LEFT JOIN` / `LEFT JOIN LATERAL` / `FULL OUTER JOIN` column as `T` instead of `Option<T>` |
| Typestates | `ProjectionPresent`, `ExecutableMutation`, `AssignmentsPresent`, `InsertSource`, `EmptyRows`, `NoBranches` | a `SELECT` without a projection; an `UPDATE` / `DELETE` without `WHERE` and without an explicit `.allow_all()`; an `UPDATE` without `SET`; an `INSERT` without a row source; a `CASE` without a `WHEN` branch |
| Dialect capabilities | `SupportsReturning`, `SupportsOnConflict`, `SupportsLateralJoin`, `SupportsFullOuterJoin`, `SupportsRowLocking`, `SupportsDistinctOn`, `SupportsAggregateFilter`, `SupportsWindowFunctions`, `SupportsNullsOrdering`, `SupportsGreatest`, `SupportsDistinctFrom`, `SupportsUpdateFrom`, `SupportsDeleteUsing`, `SupportsJsonAggregation` | `RETURNING` below the SQLite version threshold, `ON CONFLICT` below the threshold, `LATERAL` and `DELETE ... USING` on SQLite at all, `FOR UPDATE` on SQLite |
| Subquery typing in `IN` | `InSubquery<Row>`, `AnySubquery` | a subquery whose arity or projection types differ from what the predicate expects |
| Aggregation soundness | `GroupedSelectTarget`, `ColumnInGroupBy`, `AggregateInDialect`, `FunctionOperand` / `CombineAggr` | projecting an ungrouped column, including one hidden inside `coalesce`, `nullif`, `greatest`, `least`, or `CASE` |

The diagnostic is formulated in SQL terms rather than trait terms:

```rust
// rurqe::prelude
let _ = select_from::<Postgres, User>().select((UserId, OrderTotal));
```

```text
error[E0277]: column `OrderTotal` cannot be projected from this query's scope
  = note: introduce the column's table with `from` or a join before projecting it
```

## Tier 2 — pre-flight AST validation in `.build()`

Some rules require a **negative assertion** ("no such relation exists in scope") or the alignment of one clause with another clause written later in a dynamic chain. On stable Rust this can only be expressed through `#![feature(negative_impls)]`, which the project forbids. Such rules are checked deterministically in `.build()`, before any network I/O.

- The check runs over the borrowed `StatementRef` representation, that is, over the clause vectors the builder already owns. No deep AST copy is performed.
- `SelectAst::validate` delegates to the same representation, so nested statements inside expressions are checked too.
- A failure is returned as a typed `BuildError` variant and printed as a localized diagnostic that names the exact position.

### `BuildError` classes

| Group | Variants | Meaning |
|---|---|---|
| Relations | `DuplicateRelation`, `DuplicateCte`, `AliasedDerivedRelation` | the same relation (or CTE) name occurs twice; a derived relation is addressed under a name other than the one its subquery was registered with |
| Column references | `ColumnWithoutRelation`, `UnresolvedColumn`, `UnresolvedQualifier` | a column reference in a clause without a namespace (`VALUES`, `ORDER BY` of a compound query), outside the scope and outside the correlation, or a qualifier that no relation of the query declares |
| Duplication and locking | `DistinctOnOrderMismatch`, `DistinctOrderNotProjected`, `DistinctOnWithLock`, `DistinctWithLock`, `NestedLock` | the `ORDER BY` prefix does not match `DISTINCT ON`; an `ORDER BY` item is not projected under `DISTINCT`; `DISTINCT` / `DISTINCT ON` combined with row locking; a row lock inside a nested statement |
| Compound queries | `CompoundOrderIndex`, `CompoundOrderColumn`, `CompoundOrderExpression`, `SetOperandArity` | ordering the combined rows by a position outside the projection list, by a name the projection does not produce, or by an expression; an operand with a different arity |
| `INSERT` rows | `InconsistentRows`, `MismatchedRowColumns`, `EmptyInsertRow` | a row carries a different number of values or a different column at a position; a row without assignments |
| Projection and aggregation | `EmptyProjection`, `UngroupedColumnWithAggregate`, `UngroupedHavingColumn`, `UngroupedColumn`, `AggregateInRowClause` | an empty projection list; an ungrouped column next to an aggregate, in `HAVING`, or in any clause of a grouped query; an aggregate in a clause evaluated per row |
| Raw SQL | `RawSql(RawSqlError)` | a mismatch between the fragment count and the parameter count (`MixedPlaceholders`, `PlaceholderCount`) |

The aggregate position is described by the `RowClause` enumeration: `Where`, `GroupBy`, `JoinOn`, `Assignment`, `ConflictTarget`, `ConflictUpdate`. Each of these clauses is evaluated before rows are folded, so an aggregate in them is semantically impossible.

### Example

```rust
// Two relations share one effective name in scope.
let err = select_from(Users)
    .inner_join_table(Users, Expr::column::<UserId>().eq(1i64))
    .select((UserId,))
    .build()
    .unwrap_err();

assert!(matches!(err, BuildError::DuplicateRelation { name: "users" }));
```

The remedy is an alias: `let u2 = Users.as_alias("u2");` or `table_alias!`. Why the rule lives on the second level is explained in [Disjointness](../theory/disjointness.md).

## The boundary between the levels

```text
valid query ──► Tier 1 (rustc) ──► Tier 2 (.build()) ──► SQL ──► database engine
                     │                     │
                     └── compile error     └── BuildError
```

- **Tier 1 is not relaxed for convenience.** If an invariant can be expressed positively, it must live in the types.
- **Tier 2 does not duplicate Tier 1.** It covers exactly those rules that need a negative check or the alignment of clauses written at different times.
- **Panics are forbidden on both levels.** Any problem is a `BuildError`, an `ExecutionError`, or a compilation refusal, never a `panic!` / `unwrap()` in production code.

## What is not covered

Errors coming from the engine itself on a valid query (a constraint violation, missing privileges, a value type mismatch in a parameter) are admissible and are returned as `ExecutionError`, preserving the driver error, which is reachable through `as_backend()` / `into_backend()` and distinguishable through `is_not_found()`, `is_multiple_rows()`, `is_row_decode()`, and `is_build()`.
