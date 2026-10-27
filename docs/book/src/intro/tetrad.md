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

- The dialect is pinned once by a prelude (`use rurqe::postgres::prelude::*;`), a table is passed **by value** (`select_from(User)`), and columns are zero-sized values — so there is no turbofish anywhere in application code.
- **Named projections.** `select!` synthesizes an anonymous row struct at the call site, `cols!` keeps a positional selection when a tuple is genuinely wanted, and `dto!` produces a reusable struct for results that cross a function boundary. No `row.0`, no manual `FromRow` boilerplate.
- `rust-analyzer` works without a database connection: the whole schema is encoded in types.

```rust
use rurqe::postgres::prelude::*;

let contacts = select_from(User)
    .select(cols!(UserId, UserName as contact_email))
    .where_(UserId.gt(10i64))
    .order_by_asc(UserId)
    .fetch_all(&mut conn)?;
```

## 2. jOOQ expressiveness and relational depth

- **The schema is the primary source.** The catalog is introspected and the markers are generated from it; the schema is never duplicated by hand.
- **Relational expressiveness.** Multi-table joins, subqueries, CTEs (`WITH`), window functions, aggregations, `CASE WHEN`, `LATERAL`, `FULL OUTER JOIN`, and the set operations `UNION` / `INTERSECT` / `EXCEPT`.
- **Relational invariants in the types.** Outer joins lift columns into `Option<T>` automatically (see [Nullability](../theory/nullability.md)).

```rust
use rurqe::postgres::prelude::*;

let ranked = select_from(Employee)
    .select(select!(
        employee_id: EmployeeId,
        salary: Salary,
        position: rank().over(Window::partition_by(DepartmentId).order_by_desc(Salary)),
    ))
    .fetch_all(&mut conn)?;
```

## 3. Flyway lifecycle (schema evolution integrity)

- **Two models at once.** Classic sequential migrations (`V...`, `R...` with SHA-256 checksums, `baseline`, `repair`) and the AI-first **Stage + Tail** model: the declarative state under `schema/` as the SSOT for humans and LLMs, plus the active tail under `migrations/` as a delta for already deployed databases.
- **Cold start.** A clean environment (CI, container, local machine) initializes directly from `schema/`, without replaying the migration history.
- **Checkpoint and pruning.** `checkpoint.toml` records the pruning boundary and the base stage hash; files below it are deleted without a squash and without rewriting the baseline.
- **The Golden Equation.** Continuous verification: `State(schema/) == State(Checkpoint + migrations/)`, compared as the catalogs of two ephemeral databases. A divergence fails CI rather than production.
- **Fail-safe for lagging databases.** A target older than the checkpoint boundary stops the runner with a diagnostic and an instruction instead of corrupting state.

## 4. The Rust type system and zero cost

- **Zero-cost abstractions.** Monomorphized builders and phantom markers collapse into a static SQL buffer and direct deserializers.
- **Capability gates.** A clause the engine cannot offer is a method that does not exist: `RETURNING`, `ON CONFLICT`, `DISTINCT ON`, `LATERAL`, `FULL OUTER JOIN`, `UPDATE ... FROM`, `DELETE ... USING`, `FOR UPDATE` are all gated by the dialect and its version.
- **Diagnostics for humans and LLMs.** The compiler message names the violated SQL rule and the fix, not the trait that failed.

```text
// SQLite serialises writers through the database file, so a row lock is not a
// thing it can offer — and the method is absent rather than rejected at runtime:
//
//   use rurqe::sqlite::sqlite3_35::prelude::*;
//   select_from(Job).select(cols!(JobId)).for_update();   // does not compile
```

## Workspace map

| Crate | Responsibility |
|---|---|
| `rurqe` | Facade: preludes, dialect preludes, `select_from` / `insert_into` / `update` / `delete_from` |
| `rurqe-core` | AST, schema markers (`Table`, `Column`), dialect and capability traits, expressions, DML, validation |
| `rurqe-macros` | `select!`, `cols!`, `dto!`, `sql!`, `raw_sql!`, `#[derive(SqlType)]` |
| `rurqe-postgres` | PostgreSQL dialect (`Postgres<Pg14>` / `Pg15` / `Pg16`), version markers, sync and async adapters, pools |
| `rurqe-sqlite` | SQLite dialect (`Sqlite<Sqlite3_35>` and others), the `rusqlite` adapter, the `r2d2` pool |
| `rurqe-migrate` | Migration engine, schema catalog, checksums, checkpoints, stages |
| `rurqe-cli` | Catalog introspection, code generation, drift verification, stage and prune tooling |

## What this means for reading the book

- Ergonomics first: [Quickstart in five minutes](quickstart.md).
- Why the guarantees hold: [Two-tier SQL correctness](two_tier.md).
- Relational depth and theory: [Relational theory overview](../theory/overview.md).
- Dialects, runtime, migrations and the toolchain each have their own part of the book.
