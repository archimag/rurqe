# Two-Tier SQL Correctness

The promise "valid SQL or a compile error" cannot be expressed entirely inside the type system of stable Rust. RURQE therefore validates a statement at two gates, and both run before the statement is sent: the **compiler** answers every rule that can be stated as a positive assertion ("this relation is in scope"), and **validation inside `.build()`** answers the rules that need a negative assertion ("no relation under this name exists") or that align one clause with another clause written later in the fluent chain. A statement that passes both gates is valid for the dialect and the pinned engine version.

## Gate 1: the compiler

| SQL rule | What the compiler does |
|---|---|
| Every referenced relation is declared by `FROM` or a join | rejects a projection, a filter, an ordering, a grouping, a join condition, an assignment, or a `RETURNING` item that reads a table the statement never introduces |
| A `LEFT JOIN` column may be `NULL` | lifts the field of the row type to `Option<T>`; reading it as `T` does not compile |
| `UPDATE` / `DELETE` must be filtered | the statement does not build until `.where_(...)` or a deliberate `.allow_all()` is written |
| A `SELECT` projects, an `UPDATE` assigns, an `INSERT` has rows | the terminal call does not exist before the clause is written |
| The engine must support the clause | an unsupported clause has no method at all: `.for_update()` on SQLite, `RETURNING` below SQLite 3.35 |

The message is phrased in SQL terms rather than in type-system terms:

```rust,compile_fail
# extern crate rurqe;
# use rurqe::postgres::prelude::*;
# table! { User => "users" { UserId => "id": i64, } }
# table! { Order => "orders" { OrderTotal => "total": f64, } }
# fn main() {
// rejected by the compiler: `OrderTotal` belongs to a table the query never adds
let _ = select_from(User).select(cols!(UserId, OrderTotal));
# }
```

```text
error[E0277]: column `OrderTotal` cannot be projected from this query's scope
  = note: introduce the column's table with `from` or a join before projecting it
```

## Gate 2: `.build()`

Two classes of rule are out of reach of a positive assertion: one that states an **absence**, and one that compares a clause with another clause written later in the chain (`DISTINCT ON` against the leading `ORDER BY`). They are validated deterministically while the statement is assembled, and the refusal is a typed value:

```rust,no_run
# extern crate rurqe;
# use rurqe::postgres::prelude::*;
# table! { User => "users" { UserId => "id": i64, UserName => "name": String, } }
# fn main() {
// Refused while building, before the statement reaches the engine: `DISTINCT ON`
// deduplicates the rows on one expression while the leading `ORDER BY` criterion
// names another, so no engine would accept the statement.
let rejected = select_from(User)
    .select(cols!(UserId, UserName))
    .distinct_on(UserName)
    .order_by_asc(UserId)
    .build();

assert!(rejected.is_err());
# }
```

| Rule | Typed failure |
|---|---|
| `DISTINCT ON (x)` must lead the `ORDER BY` with the same expression | `DistinctOnOrderMismatch` |
| `DISTINCT` must order by an expression the projection produces | `DistinctOrderNotProjected` |
| A compound statement must order by a name or a position it produces | `CompoundOrderColumn`, `CompoundOrderIndex` |
| Two relations must not share one effective name | `DuplicateRelation` |
| A raw SQL fragment must carry as many placeholders as it has parameters | `RawSql` |

The duplicate relation is the case where the two gates meet: two occurrences of the same unaliased table share one relation name, so the statement is refused with the colliding name, and the remedy is an alias — `users.as_alias("u2")` — which gives the second occurrence a name of its own.

## Which gate catches which mistake

| Gate | When it runs | What it catches | What the caller sees |
|---|---|---|---|
| Compiler | `cargo build` | a rule stated as a positive assertion: a relation in scope, a nullable join side, a mutation typestate, a capability the engine lacks | a compile error |
| Statement validation | `.build()` | a rule that needs an absence check or a cross-clause alignment | a `BuildError` |
| Database engine | execution | a failure of a valid statement: a constraint violation, a missing privilege, a parameter the server refuses | an `ExecutionError` |

- **The first gate is not relaxed for convenience.** If an invariant can be expressed positively, it lives in the types.
- **The second gate does not duplicate the first.** It covers exactly the rules that need an absence check or a cross-clause alignment.
- **Neither gate panics.** A problem is a compile error, a `BuildError`, or an `ExecutionError`, never a panic on the way.

## Errors that come from the engine

A failure of a valid statement — a constraint violation, missing privileges, a parameter the server refuses — is admissible and is returned as an `ExecutionError`, preserving the driver error. It stays inspectable: `is_not_found()`, `is_multiple_rows()`, `is_row_decode()`, `is_build()`, `as_backend()`, `into_backend()`.

The full catalogue of rejected statements is in the [error reference](../appendix/compiler_errors.md).
