# Two-Tier SQL Correctness

The promise "valid SQL or a compile error" cannot be expressed entirely inside the open type system of stable Rust. RURQE therefore checks a statement at two gates: the **compiler** answers every rule that can be stated as a positive assertion ("this relation is in scope"), and `.build()` answers the few rules that need a negative one ("no relation under this name exists") or the alignment of a clause with another clause written later in a fluent chain. Both gates run before any statement reaches the database.

## Gate 1: the compiler

| SQL rule | What the compiler does |
|---|---|
| Every referenced relation is declared by `FROM` or a join | rejects a projection, filter, ordering, grouping, join condition, assignment, or `RETURNING` item that reads a table the statement never introduces |
| A `LEFT JOIN` column may be `NULL` | lifts the field of the row type to `Option<T>`; reading it as `T` does not compile |
| `UPDATE` / `DELETE` must be filtered | the statement does not build until `.where_(...)` or a deliberate `.allow_all()` is written |
| A `SELECT` projects, an `UPDATE` assigns, an `INSERT` has rows | the terminal call does not exist before the clause is written |
| The engine must support the clause | an unsupported clause has no method at all: `.for_update()` on SQLite, `RETURNING` below SQLite 3.35 |

The message is phrased in SQL terms rather than in trait terms:

```rust
// rejected by the compiler: `OrderTotal` belongs to a table the query never adds
let _ = select_from(Users).select(select!(UserId, OrderTotal));
```

```text
error[E0277]: column `OrderTotal` cannot be projected from this query's scope
  = note: introduce the column's table with `from` or a join before projecting it
```

## Gate 2: `.build()`

Two classes of rule are out of reach of a positive assertion on stable Rust: one that states an **absence**, and one that compares a clause with another clause written later in the chain (`DISTINCT ON` against the leading `ORDER BY`). They are validated deterministically while the statement is assembled:

```rust
// Rejected while building, before the statement reaches the engine: the same
// relation is addressed twice under one name.
let error = select_from(Users)
    .inner_join(Users, Expr::column::<UserId>().eq(1i64))
    .select(cols!(UserId))
    .build()
    .unwrap_err();

assert!(matches!(error, BuildError::DuplicateRelation { name: "users" }));
```

The remedy is an alias — `let u2 = Users.as_alias("u2");`, see [Disjointness](../theory/disjointness.md). The same gate covers a `DISTINCT ON` whose `ORDER BY` prefix does not match, a `DISTINCT` that orders by an expression the projection does not produce, the ordering of a compound statement, and a raw SQL fragment whose parameter count disagrees with its placeholders. Every failure is a typed `BuildError` and the network is never touched; the full catalogue lives in the [error reference](../appendix/compiler_errors.md).

## The boundary between the gates

```text
valid query ──► compiler ──► .build() ──► SQL ──► database engine
                    │            │
                    │            └── BuildError
                    └── compile error
```

- **The first gate is not relaxed for convenience.** If an invariant can be expressed positively, it lives in the types.
- **The second gate does not duplicate the first.** It covers exactly the rules that need an absence check or a cross-clause alignment.
- **Neither gate panics.** A problem is a compile error, a `BuildError`, or an `ExecutionError` — never an `unwrap()` in production code.

## Errors that come from the engine

A failure of a valid statement — a constraint violation, missing privileges, a parameter that the server refuses — is admissible and is returned as `ExecutionError`, preserving the driver error. It stays inspectable: `is_not_found()`, `is_multiple_rows()`, `is_row_decode()`, `is_build()`, `as_backend()`, `into_backend()`.
