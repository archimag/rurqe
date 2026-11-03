# Two-Tier SQL Correctness

The promise "valid SQL or a compile error" cannot be expressed entirely inside the type system of stable Rust. RURQE validates a statement at two gates before any SQL reaches the database engine:

1. **The compiler (Gate 1)** enforces relational invariants expressible as positive assertions: relation scope isolation, outer-join nullability lifting, typestates, dialect capability gating, and the grouping coverage of a query that grouped its rows.
2. **Validation inside `.build()` (Gate 2)** enforces the rules that need a negative assertion (absence of duplicate unaliased relations), a cross-clause alignment (`DISTINCT ON` prefix matching leading `ORDER BY`), or a statement-level aggregation the builder typestates never saw: an aggregate projected beside a bare column without a grouping (`UngroupedColumnWithAggregate`), and the ordering, deduplication, and `HAVING` clauses of a collapsed statement.

A statement that passes both gates is guaranteed valid for the target dialect and engine version. Both gates decide the statement before any I/O: assembling a query performs no I/O and carries no runtime or connection type, so the terminal method alone determines whether execution is awaited or synchronous.

## Gate 1: The Compiler

Compile-time type soundness prevents syntactically and semantically invalid query structures at build time:

| SQL rule | What the compiler does |
|---|---|
| Every referenced relation is declared by `FROM` or a join | Rejects projections, filters, orderings, groupings, join conditions, assignments, and `RETURNING` clauses reading an unjoined table |
| A `LEFT JOIN` column may be `NULL` | Automatically lifts the column type to `Option<T>`; reading it as an unwrapped scalar fails to compile |
| `UPDATE` / `DELETE` must be filtered | Mutation statements require `.where_(...)` or an explicit `.allow_all()` before execution methods exist |
| A `SELECT` projects, an `UPDATE` assigns, an `INSERT` has rows | Terminal calls do not exist until required statement clauses are written |
| A grouped query projects only grouped columns, aggregates, constants, and windows | After `.group_by(...)` the projection, the `HAVING` predicate, and the window specification are checked against the recorded grouping at the call site |
| The engine must support the clause | Unsupported clauses have no methods: `.for_update()` on SQLite, `RETURNING` below SQLite 3.35 |
| A raw SQL template binds exactly one parameter per declared placeholder | The `sql!` / `raw_sql!` macros refuse a template whose placeholder count disagrees with the parameters it is given while the macro expands |
Compiler error messages describe the violated SQL rule directly rather than leaking internal generic type cascades:

```rust,compile_fail
{{#rustdoc_include ../examples/guards/queries.rs:out_of_scope_projection}}
```

```text
error[E0277]: column `OrderTotal` cannot be projected from this query's scope
  = note: introduce the column's table with `from` or a join before projecting it
```

## Gate 2: Pre-Flight Validation in `.build()`

Rules that require checking the absence of a relation or comparing multi-clause alignments are evaluated deterministically in `.build()` before any network dispatch. Rejections produce typed `BuildError` values:

```rust,no_run
{{#rustdoc_include ../examples/quickstart.rs:build_refusal}}
```

| Rule | Typed failure |
|---|---|
| `DISTINCT ON (x)` must lead the `ORDER BY` with the same expression | `DistinctOnOrderMismatch` |
| `DISTINCT` must order by an expression the projection produces | `DistinctOrderNotProjected` |
| A compound statement must order by a name or a position it produces | `CompoundOrderColumn`, `CompoundOrderIndex` |
| A set operation must combine operands projecting the same number of columns | `SetOperandArity` |
| Two relations must not share one effective name | `DuplicateRelation` |
| An assembled raw SQL expression carries as many placeholders as it has parameters | `RawSql` |
| An ungrouped statement projects an aggregate beside a bare column | `UngroupedColumnWithAggregate` |
| A collapsed statement orders or deduplicates by a column the collapse dropped | `UngroupedColumn` |
| A `HAVING` predicate reads a column that is neither aggregated nor grouped | `UngroupedHavingColumn` |
| A mutation's `RETURNING` projection reads an aggregate or a window call | `AggregateInRowClause`, `WindowInRowClause` |
| An `ON CONFLICT` clause records a `DO UPDATE` action or a target predicate without a conflict target | `MissingConflictTarget` |

The grouping rules sit on both gates, and the split follows the two contracts. A query that grouped its rows through `.group_by(...)` is a grouped statement at the call site, so a projection, a `HAVING` predicate, or a window specification reading a column the grouping dropped is refused by the compiler. The three grouping variants above are the residuals the second gate owns: a statement that projects an aggregate beside a stored column without grouping its rows carries no grouped typestate, so it compiles and is refused when it is built, and the ordering, the `DISTINCT ON` expressions, and the `HAVING` predicate are re-checked on the assembled statement before any SQL is rendered.
When two occurrences of the same unaliased table share a relation name in `FROM` and `JOIN`, `.build()` rejects the query with `DuplicateRelation`. Introducing an explicit table alias resolves the conflict: `table_alias!` declares the alias marker, and `User.as_alias(Manager)` hands the aliased relation to the builder.

## Verification Gates Comparison

| Gate | Execution timing | Evaluated invariants | Caller outcome |
|---|---|---|---|
| Compiler | `cargo build` | Positive assertions: relation scope, nullable joins, mutation typestates, dialect capability gates, grouped query projections | Compile-time error |
| Statement validation | `.build()` | Negative assertions (duplicate unaliased relations), cross-clause alignments (`DISTINCT ON`), and residual aggregation checks (`UngroupedColumnWithAggregate`) | `Result<BuiltQuery, BuildError>` |
| Database engine | Terminal query call | Server-side runtime errors: constraint violations, missing permissions, invalid connection state | `Result<T, ExecutionError>` |

## Engine Errors and Diagnostics

Failures originating from the database engine (unique violations, foreign key conflicts, lock timeouts) are returned as `ExecutionError` preserving the underlying driver error. Diagnostic helpers provide clean classification without string inspection:

* `is_not_found()`: Query yielded zero rows when expecting at least one.
* `is_multiple_rows()`: Single-row query returned multiple matching rows.
* `is_row_decode()`: Column value failed deserialization into the requested Rust type.
* `is_build()`: Pre-flight AST validation rejected the query.
* `as_backend()` and `into_backend()`: Inspect or unwrap the native driver error.
