# The architectural tetrad

RURQE is assembled from four industrial ideas, each of which closes a weakness of the others.

```text
┌──────────────────────────────────────────────────────────────────────────────┐
│                            THE ARCHITECTURAL TETRAD                          │
├───────────────────────┬────────────────────────┬───────────────────┬─────────┤
│ Kysely ergonomics     │ jOOQ expressiveness    │ Flyway lifecycle  │ Rust    │
│        (DX)           │ and relational depth   │                   │ types   │
├───────────────────────┼────────────────────────┼───────────────────┼─────────┤
│ • fluent syntax       │ • schema as SSOT       │ • versioned V...  │ • gates │
│ • named fields        │ • complex joins, CTEs  │ • repeatable R... │ • zero  │
│ • zero boilerplate    │ • strict nullability   │ • SHA-256         │   cost  │
│ • autocompletion      │ • catalog introspection│ • baseline/repair │ • clear │
│                       │                        │ • pruning         │  errors │
└───────────────────────┴────────────────────────┴───────────────────┴─────────┘
```

## 1. Kysely ergonomics (developer experience)

- A query reads like SQL and needs no turbofish at every step: the dialect is pinned by a prelude (`rurqe::postgres::prelude::*`), a table is passed by value (`select_from(Users)`), and columns are ZST values.
- Named projections: `select!` synthesizes a local row struct, `dto!` synthesizes a reusable one, instead of tuple indices `row.0` / `row.1`.
- `rust-analyzer` works without a database connection: all schema information is encoded in types.

An example of the shape (verified against real code in the [Quickstart](quickstart.md)):

```text
use rurqe::postgres::prelude::*;

select_from(Users)
    .select(cols!(UserId, UserName))
    .where_(UserId.gt(10i64))
```

## 2. jOOQ expressiveness and relational depth

- **The schema is the primary source.** The database catalog is introspected, markers and the `schema.lock` snapshot are generated from it, and the schema is never duplicated by hand.
- **Relational expressiveness.** Multi-table `JOIN`s, subqueries, CTEs (`WITH`), window functions, aggregations, `CASE WHEN`, `LATERAL`, `FULL OUTER JOIN`, and the set operations `UNION` / `INTERSECT` / `EXCEPT`.
- **Relational invariants in the types.** Outer joins lift columns into `Option<T>` automatically (see [Nullability](../theory/nullability.md)).

## 3. Flyway lifecycle (schema evolution integrity)

- **Two models at once.** Classic sequential migrations (`V...`, `R...` with SHA-256 checksums, `baseline`, `repair`) and the AI-first **Stage + Tail** model: the declarative state under `schema/` as the SSOT for humans and LLMs, plus the active tail under `migrations/` as a delta for already deployed databases.
- **Cold start.** A clean environment (CI, container, local machine) is initialized directly from `schema/` without replaying the migration history.
- **Checkpoint and pruning.** The `checkpoint.toml` marker records the `pruned_up_to_version` boundary and the base stage hash; historical files below that boundary are deleted without a squash and without rewriting the baseline.
- **The Golden Equation.** Continuous verification: `State(schema/) == State(Checkpoint + migrations/)`, compared as the catalogs of two ephemeral databases. A divergence is caught in CI rather than in production.
- **Fail-safe for lagging databases.** If the target database is older than the pruned checkpoint boundary, the runner stops with a diagnostic and an instruction instead of corrupting the state.

## 4. The Rust type system and zero-cost

- **Zero-cost abstractions.** Monomorphized builders and phantom markers collapse into a static SQL buffer and direct deserializers.
- **Compile-time capability gates.** `RETURNING`, `ON CONFLICT`, `DISTINCT ON`, `LATERAL`, `FULL OUTER JOIN`, `UPDATE ... FROM`, `DELETE ... USING`, and `FOR UPDATE` are marker traits (`SupportsReturning`, `SupportsOnConflict`, `SupportsDistinctOn`, `SupportsLateralJoin`, `SupportsFullOuterJoin`, `SupportsUpdateFrom`, `SupportsDeleteUsing`, `SupportsRowLocking`, ...). No trait implementation means no method.
- **Diagnostics for humans and for LLMs.** `#[diagnostic::on_unimplemented]` turns a trait cascade into one short message with a hint.

## Workspace map

| Crate | Responsibility |
|---|---|
| `rurqe` | Facade: preludes, dialect preludes, the `select_from` / `insert_into` / `update` / `delete_from` entry points |
| `rurqe-core` | AST, schema markers (`Table`, `Column`), dialect and capability traits, expressions, DML, validation |
| `rurqe-macros` | Procedural macros `select!`, `cols!`, `dto!`, `sql!`, `raw_sql!`, `#[derive(SqlType)]` |
| `rurqe-postgres` | PostgreSQL dialect (`Postgres<Pg14>` / `Pg15` / `Pg16`), version markers, synchronous and asynchronous driver adapters, pools |
| `rurqe-sqlite` | SQLite dialect (`Sqlite<Sqlite3_35>` and others), the `rusqlite` adapter, the `r2d2` pool |
| `rurqe-migrate` | Migration engine, schema catalog, checksums, checkpoints, stages |
| `rurqe-cli` | Catalog introspection, code generation, drift verification, stage and prune tooling |

## What this means for reading the book

- Ergonomics: [Quickstart](quickstart.md).
- Correctness: [Two-Tier SQL Correctness](two_tier.md) and the whole [theory part](../theory/overview.md).
- Dialects: the PostgreSQL and SQLite chapters (separate sections of the book).
- Migrations: the chapters about the traditional path and Stage + Tail (separate sections of the book).
