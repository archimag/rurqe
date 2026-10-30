# Two-Tier SQL Correctness

The promise "valid SQL or a compile error" cannot be expressed entirely inside the type system of stable Rust. RURQE validates a statement at two gates before any SQL reaches the database engine:

1. **The compiler (Gate 1)** enforces relational invariants expressible as positive assertions: relation scope isolation, outer-join nullability lifting, typestates, and dialect capability gating.
2. **Validation inside `.build()` (Gate 2)** enforces rules requiring negative assertions (absence of duplicate unaliased relations) or cross-clause alignment (`DISTINCT ON` prefix matching leading `ORDER BY`).

A statement that passes both gates is guaranteed valid for the target dialect and engine version. Neither gate touches the runtime: query construction is runtime-orthogonal, and the terminal method alone determines whether execution is awaited or synchronous.

## Gate 1: The Compiler

Compile-time type soundness prevents syntactically and semantically invalid query structures at build time:

| SQL rule | What the compiler does |
|---|---|
| Every referenced relation is declared by `FROM` or a join | Rejects projections, filters, orderings, groupings, join conditions, assignments, and `RETURNING` clauses reading an unjoined table |
| A `LEFT JOIN` column may be `NULL` | Automatically lifts the column type to `Option<T>`; reading it as an unwrapped scalar fails to compile |
| `UPDATE` / `DELETE` must be filtered | Mutation statements require `.where_(...)` or an explicit `.allow_all()` before execution methods exist |
| A `SELECT` projects, an `UPDATE` assigns, an `INSERT` has rows | Terminal calls do not exist until required statement clauses are written |
| The engine must support the clause | Unsupported clauses have no methods: `.for_update()` on SQLite, `RETURNING` below SQLite 3.35 |

Compiler error messages describe the violated SQL rule directly rather than leaking internal generic type cascades:

```rust,compile_fail
use rurqe::postgres::prelude::*;

table! {
    User => "users" {
        UserId => "id": i64,
    }
}

table! {
    Order => "orders" {
        OrderTotal => "total": f64,
    }
}

// Rejected by the compiler: `OrderTotal` belongs to a table the query never introduces
let _ = select_from(User).select(cols!(UserId, OrderTotal));
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
| A set operation must combine operands projecting the same number of columns | `SetOperandArity` || Two relations must not share one effective name | `DuplicateRelation` |
| A raw SQL fragment must carry as many placeholders as it has parameters | `RawSql` |

When two occurrences of the same unaliased table share a relation name in `FROM` and `JOIN`, `.build()` rejects the query with `DuplicateRelation`. Introducing an explicit table alias (`table.as_alias("u2")`) resolves the conflict.

## Verification Gates Comparison

| Gate | Execution timing | Evaluated invariants | Caller outcome |
|---|---|---|---|
| Compiler | `cargo build` | Positive assertions: relation scope, nullable joins, mutation typestates, dialect capability gates | Compile-time error |
| Statement validation | `.build()` | Negative assertions (duplicate unaliased relations) and cross-clause ordering alignments | `Result<BuiltQuery, BuildError>` |
| Database engine | Terminal query call | Server-side runtime errors: constraint violations, missing permissions, invalid connection state | `Result<T, ExecutionError>` |

## Engine Errors and Diagnostics

Failures originating from the database engine (unique violations, foreign key conflicts, lock timeouts) are returned as `ExecutionError` preserving the underlying driver error. Diagnostic helpers provide clean classification without string inspection:

* `is_not_found()`: Query yielded zero rows when expecting at least one.
* `is_multiple_rows()`: Single-row query returned multiple matching rows.
* `is_row_decode()`: Column value failed deserialization into the requested Rust type.
* `is_build()`: Pre-flight AST validation rejected the query.
* `as_backend()` and `into_backend()`: Inspect or unwrap the native driver error.
