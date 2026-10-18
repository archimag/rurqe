# RURQE: Rust Universal Relational Query Engine
## Project Roadmap & Architecture Specification

**RURQE** (**R**ust **U**niversal **R**elational **Q**uery **E**ngine) is an ergonomic, strictly type-safe SQL query builder, relational execution engine, and database-first codegen toolkit for Rust. It synthesizes the architectural strengths of **jOOQ** (database-first single source of truth, relational depth, sound nullability) and **Kysely** (zero-boilerplate ergonomics, named ad-hoc row projections, instant IDE feedback) with the zero-cost guarantees and compile-time invariants of **Rust**.

The roadmap is structured into vertical slices: **at the end of each phase, the library is fully functional, cohesive, and immediately useful**.

---

## Workspace Crate Architecture

RURQE is designed as a modular Cargo workspace, enabling minimal dependency footprints and fast compilation:

- **`rurqe`**: The primary facade crate providing high-level re-exports and fluent entrypoints.
- **`rurqe-core`**: Pure, lightweight AST representation, schema markers (`Table`, `Column`), dialect traits (`Dialect`), capability marker traits, and expression operator logic.
- **`rurqe-macros`**: Declarative and procedural macros (`select!`, `dto!`, `table!`) for zero-cost projection synthesis and compile-time schema DSLs.
- **`rurqe-postgres`**: PostgreSQL dialect renderer, version markers (`Pg14`, `Pg15`, `Pg16`), and execution adapters for `tokio-postgres` and synchronous `postgres`.
- **`rurqe-sqlite`**: SQLite dialect renderer, version bounds (`Sqlite3_35`, etc.), and driver adapter for `rusqlite`.
- **`rurqe-cli`**: Developer toolchain for live catalog introspection, code generation, migration execution, and CI schema drift verification (`schema.lock`).

---

## Core Vision & Design Philosophy: The Foundational Triad

The guiding north star of this library is the synthesis of three superpowers into a unified, uncompromising developer experience:

```text
┌────────────────────────────────────────────────────────────────────────┐
│                        THE FOUNDATIONAL TRIAD                          │
├───────────────────────┬────────────────────────┬───────────────────────┤
│   Kysely Ergonomics   │  jOOQ Expressiveness   │    Rust Type Safety   │
│         (DX)          │   & Relational Depth   │ & Zero-Cost Guarantees│
├───────────────────────┼────────────────────────┼───────────────────────┤
│ • Fluent syntax       │ • Database-first truth │ • Compile-time guards │
│ • Named projections   │ • Complex joins & CTEs │ • Zero runtime cost   │
│ • Zero boilerplate    │ • Schema drift verify  │ • Actionable errors   │
│ • First-class tooling │ • Sound nullability    │ • Multi-dialect gates │
└───────────────────────┴────────────────────────┴───────────────────────┘
```

### 1. Kysely Ergonomics (Developer Experience)
- **Named Ad-Hoc Projections:** Ergonomic projection macros (`select!`) synthesize strongly typed row records with named fields (`user.id`, `user.email`), eliminating tuple indexing (`row.0`, `row.1`) without tedious manual DTO definitions.
- **Zero-Boilerplate Fluent API:** Queries read like idiomatic, clean SQL with minimal syntactic overhead and no generic type noise.
- **First-Class Tooling:** Instant autocompletion, inlay hints, and rename-refactoring support in `rust-analyzer` without requiring a running database server during compilation.

### 2. jOOQ Expressiveness & Relational Soundness
- **Database-First Single Source of Truth:** Schema metadata is introspected directly from the live database catalog, eliminating duplicated hand-written schema definitions and drift.
- **Relational Expressiveness:** First-class support for advanced SQL capabilities: multi-table joins, subqueries, CTEs (`WITH`), window functions, aggregations, and `CASE WHEN` constructs.
- **Relational Type Invariants:** Mathematically sound compile-time nullability tracking (e.g. automatically lifting non-nullable columns to `Option<T>` in `LEFT JOIN` and outer join operations).
- **Lifecycle Integrity & Baseline:** Comprehensive migration engine, baseline snapshots for legacy and squashed schemas, and deterministic CI drift detection via `schema.lock`.

### 3. Rust Strict Type Safety & Zero-Cost Guarantees
- **Zero-Cost Abstractions:** Monomorphized query builders and phantom markers compile down to static SQL buffers and direct deserializers with zero virtual calls or reflection overhead.
- **Compile-Time Capability & Version Gating:** Dialect- and version-dependent SQL features (such as `RETURNING`, `MERGE`, or `DISTINCT ON`) are enforced via capability marker traits; incompatible queries simply fail to compile.
- **Human- and LLM-Friendly Diagnostics:** Explicit diagnostics via `#[diagnostic::on_unimplemented]` and fail-fast call-site checking ensure short, actionable compiler error messages rather than multi-clause trait cascade dumps.

---

## Evolution Overview

| Phase | Focus | User Capability | Product Maturity |
|---|---|---|---|
| **Phase 1** | Type-Safe Core & AST | Strict `SELECT` with tuple mapping (Postgres first, dialect-ready) | Minimal Viable Query Builder |
| **Phase 2** | Kysely DX & Macros | Named fields via `select!`, DTO derivation | High-Ergonomics Read Layer |
| **Phase 3** | Type-Safe DML | `INSERT`, `UPDATE`, `DELETE`, `RETURNING` | Complete CRUD Solution |
| **Phase 4** | Relational Power | Joins, compile-time nullability, group by | Advanced Relational Builder |
| **Phase 5** | Codegen, Migrations & Drift Verification | CLI introspection, migration engine, baseline & schema lock | Zero-Boilerplate & Verified Integrity |
| **Phase 6** | Production Hardening | Dual engine (sync/async), multi-dialect (SQLite), pools | Production-Grade Framework |

---

## Architectural Foundation: Multi-Dialect Design

To support multiple SQL engines (PostgreSQL primary, followed by SQLite and MySQL) without runtime overhead or lowest-common-denominator compromises:

### 1. Zero-Cost Dialect Abstraction
- **`Dialect` Trait:** Encapsulates dialect-specific SQL rendering rules:
  - Identifier quoting (`"table"` vs `` `table` ``).
  - Parameter placeholder format (`$1, $2` in PostgreSQL vs `?, ?` in SQLite).
  - String literal escaping and boolean representation (`TRUE/FALSE` vs `1/0`).
- **Zero Runtime Overhead:** Dialect is a zero-sized phantom type parameter on queries (e.g., `SelectQuery<Postgres, ...>`), enabling monomorphization and compiler inlining.

### 2. Compile-Time Capability Marker Traits
Rather than failing at runtime when an engine lacks a feature, SQL capabilities are verified at compile time via marker traits:
- `SupportsDistinctOn: Dialect` (PostgreSQL only)
- `SupportsReturning: Dialect` (PostgreSQL, SQLite 3.35+)
- `SupportsIlike: Dialect` (PostgreSQL)
- `SupportsOnConflict: Dialect` (PostgreSQL, SQLite 3.24+)

Methods requiring specific SQL features are gated via trait bounds:
```rust
impl<D: SupportsDistinctOn, T: Table> SelectQuery<D, T> {
    pub fn distinct_on<C: Column>(self, col: C) -> Self { ... }
}
```
Attempting to invoke `.distinct_on(...)` on a `SelectQuery<Sqlite, ...>` produces a compile error: *the trait `SupportsDistinctOn` is not implemented for `Sqlite`*.

### 3. Version-Aware Capabilities & Minimum Engine Target
Database features frequently depend on the target engine release:
- **SQLite:** `UPSERT` (3.24+), generated columns (3.31+), `RETURNING` (3.35+), `->` / `->>` JSON operators (3.38+).
- **PostgreSQL:** `jsonpath` (12+), multirange types (14+), `MERGE` statement (15+), enhanced regex / JSON constructors (16+).
- **MySQL:** CTEs and Window Functions (8.0+ vs 5.7).

To represent engine versions at zero runtime cost without fragmented dialect types:
- Dialects accept an optional type-level version parameter with modern defaults (e.g., `Postgres<V = Pg16>`, `Sqlite<V = Sqlite3_35>`).
- Capability traits are conditionally implemented based on version bounds:
  ```rust
  impl<V: SqliteVersionAtLeast<3, 35>> SupportsReturning for Sqlite<V> {}
  impl<V: PgVersionAtLeast<15>> SupportsMerge for Postgres<V> {}
  ```
- Target version is specified once in project configuration or codegen CLI (`target_version = "sqlite:3.35"`), binding the target type to the generated schema.
- Calling a feature unsupported by the configured version produces a focused compiler diagnostic via `#[diagnostic::on_unimplemented]`:
  ```text
  error[E0277]: The `RETURNING` clause requires SQLite 3.35.0+, but target dialect is configured for `Sqlite<Sqlite3_24>`
  ```

### 4. Dialect-Specific Extension Traits
Engine-exclusive syntax (such as PostgreSQL JSONB operators `@>`, `?|`, regex matching `~`, or full-text search) is exposed via dedicated extension traits (`PgExpressions`, `SqliteExpressions`) that decorate expressions only when the active dialect matches.

---

## Architectural Foundation: Compiler Diagnostics & Error Ergonomics (Human & LLM First)

A critical failure mode of heavily typed SQL libraries in Rust (such as Diesel) is opaque compiler error messages spanning dozens of lines of deeply nested AST types. To make this library effortless to understand and debug for both human developers and LLMs / AI agents:

### 1. Actionable Diagnostics via `#[diagnostic::on_unimplemented]`
Public core traits utilize Rust 1.78+ `#[diagnostic::on_unimplemented]` to deliver concise, focused error messages with remediation hints instead of low-level trait resolution failures:
```rust
#[diagnostic::on_unimplemented(
    message = "Cannot compare column `{Self}` with value of type `{Rhs}`",
    label = "type mismatch: column `{Self}` expects `{Expected}`, but got `{Rhs}`",
    note = "Hint: verify that the value type matches the column definition in your schema"
)]
pub trait SqlEq<Rhs, Expected> { ... }
```

### 2. Fail-Fast Call-Site Validation
Type errors are caught immediately at the offending method call (`.where_()`, `.eq()`, `.select()`), rather than deferred to the terminal execution method (`.fetch_all()`). This ensures the compiler points directly to the incorrect expression and prevents multi-clause error cascading.

### 3. Flat Query Types (No Generic Parameter Explosion)
Deeply nested AST wrapper types are avoided in public query signatures. The query signature exposes only its high-level public contract:
```rust
pub struct Query<Dialect, TablesInScope, OutputRow> { ... }
```
Internal clauses are encapsulated inside AST nodes rather than layered into type parameters, keeping compiler errors and rust-analyzer inlay hints short and readable.

### 4. Semantic, Human-Readable Marker Traits
Trait boundaries are formulated as natural English domain constraints rather than obscure internal mechanics:
- `ColumnBelongsTo<Table>`
- `TableInScope<Table>`
- `SupportsDistinctOn<Dialect>`
- `SqlCompatible<ColType, ValType>`

### 5. Explicit Macro Diagnostics
Declarative and procedural macros (`select!`, `dto!`, `table!`) validate syntax and invariants proactively, producing structured `compile_error!("...")` messages with explicit guidance rather than failing with confusing internal parser syntax errors.

---

## Phase 1: Type-Safe Core & Tuple Select

### Objective
Create a lightweight, strictly typed query builder core with an extensible multi-dialect architecture, capable of constructing parameterized SQL queries and decoding rows into typed tuples without runtime overhead.

### Key Deliverables
- **Multi-Dialect Foundation:**
  - `Dialect` trait specifying identifier quoting and placeholder formatting.
  - `Postgres` dialect implementation (`$1, $2, ...` placeholders, double-quote identifiers).
  - Capability marker traits infrastructure (`SupportsReturning`, `SupportsDistinctOn`, version-gated capabilities).
- **Core Abstractions:**
  - `Table` trait: table identifier and schema namespace.
  - `Column` trait: column name, parent table marker, and associated `RustType` & `SqlType`.
  - `Expression<SqlType>` trait: type-safe SQL expressions and operators (`eq`, `neq`, `gt`, `lt`, `and`, `or`).
- **Query Construction:**
  - `SelectQuery<D: Dialect, ...>` builder supporting `.from()`, `.where_()`, `.order_by()`, and `.limit()`.
  - Deterministic parameterized SQL renderer driving output via the active `Dialect`.
- **Manual Schema Declaration DSL:**
  - A lightweight declarative macro (`table!`) for defining table and column markers in under 10 lines of code per table.
- **Async Execution:**
  - Execution adapter for `tokio-postgres` decoding typed projections directly into tuples (e.g. `(i32, String)`).

### Example Usage
```rust
table! {
    users {
        id: i32,
        email: String,
        is_active: bool,
    }
}

let rows: Vec<(i32, String)> = db
    .select((users::ID, users::EMAIL))
    .from(users::TABLE)
    .where_(users::IS_ACTIVE.eq(true))
    .fetch_all(&client)
    .await?;
```

---

## Phase 2: Kysely DX: Macros for Named Projections (`select!` & DTOs)

### Objective
Eliminate tuple indexing (`row.0`, `row.1`) in favor of named fields matching Kysely's ergonomic developer experience, using Rust's macro system without requiring compile-time database connectivity.

### Key Deliverables
- **Ad-Hoc Projection Macro (`select!`):**
  - Synthesizes an anonymous local row struct at the call site.
  - Resolves field types automatically from associated column markers (`<Col as Column>::RustType`).
  - Supports field aliasing (`users::EMAIL as contact_email`).
  - Implements row decoding traits automatically for the generated struct.
- **DTO Macro (`dto!`):**
  - Generates reusable structs for queries whose results must cross function boundaries, deriving field types directly from schema columns.

### Example Usage
```rust
let users = select! {
    users::ID,
    users::EMAIL as contact_email,
}
.from(users::TABLE)
.where_(users::ID.gt(10))
.fetch_all(&client)
.await?;

for user in users {
    println!("User #{}: {}", user.id, user.contact_email);
}
```

---

## Phase 3: Type-Safe DML (Insert, Update, Delete, Returning, Upsert)

### Objective
Expand the query builder into a comprehensive CRUD library with compile-time safety guards against common runtime mistakes.

### Key Deliverables
- **Type-Safe `INSERT`:**
  - Strongly typed column assignment (`users::EMAIL.set("alice@example.com")`).
  - Support for `DEFAULT` values (e.g. autoincrement primary keys).
  - Bulk inserts (`insert_many`).
- **Compile-Time Safe `UPDATE` & `DELETE`:**
  - Typestate enforcement: prevents executing `UPDATE` or `DELETE` without a `WHERE` clause unless explicitly overridden (`.allow_all()`).
- **Dialect- and Version-Gated `RETURNING` Clause:**
  - Available where `D: SupportsReturning` (PostgreSQL, SQLite 3.35+).
  - Return individual columns, typed tuples, or complete record structs upon mutation.
- **Dialect- and Version-Aware Conflict Handling (Upsert):**
  - Standard `ON CONFLICT (columns) DO NOTHING / DO UPDATE` for dialects implementing `SupportsOnConflict` (PostgreSQL, SQLite 3.24+).
  - Dialect-specific upsert syntax variants formatted transparently.

### Example Usage
```rust
let new_id: i32 = db.insert_into(users::TABLE)
    .values((
        users::EMAIL.set("user@example.com"),
        users::IS_ACTIVE.set(true),
    ))
    .returning(users::ID)
    .fetch_one(&client)
    .await?;

db.update(users::TABLE)
    .set(users::IS_ACTIVE.set(false))
    .where_(users::ID.eq(new_id))
    .execute(&client)
    .await?;
```

---

## Phase 4: Relational Querying: Joins, Nullability Tracking & Aggregations

### Objective
Provide advanced relational querying capabilities comparable to jOOQ, maintaining sound compile-time nullability tracking across joins.

### Key Deliverables
- **Join Semantics:**
  - `INNER JOIN`, `LEFT JOIN`, `RIGHT JOIN`, and `FULL JOIN`.
- **Automatic Nullability Tracking:**
  - Type-level transformation: columns participating in a `LEFT JOIN` automatically wrap their Rust type in `Option<T>` at compile time, preventing runtime panics.
- **Aggregations & Grouping:**
  - Standard aggregate functions (`count`, `sum`, `avg`, `min`, `max`).
  - `.group_by(...)` and `.having(...)` validation.
- **Conditional Expressions:**
  - Type-safe `case_when(...).then(...).else_(...).end()`.

### Example Usage
```rust
let report = select! {
    users::ID,
    users::EMAIL,
    orders::TOTAL, // Inferred as Option<Decimal> due to LEFT JOIN!
}
.from(users::TABLE)
.left_join(orders::TABLE, orders::USER_ID.eq(users::ID))
.where_(users::IS_ACTIVE.eq(true))
.fetch_all(&client)
.await?;
```

---

## Phase 5: jOOQ-Style Codegen, Migrations & Schema Drift Verification (Baseline)

### Objective
Eliminate manual schema maintenance and guarantee total synchronization between migrations, database state, and Rust types via database-first introspection, an integrated migration engine, baseline snapshots, and deterministic drift detection.

### Key Deliverables
- **Transactional Migration Engine:**
  - Forward-only, sequential SQL migration files (`migrations/0001_init.sql`, `migrations/0002_add_orders.sql`).
  - Checksum validation: stores SHA-256 hashes in `__schema_migrations` to catch retroactive edits to already applied migrations.
  - Dual execution:
    - CLI commands: `cargo db-migrate up`, `cargo db-migrate status`.
    - Embedded migrator: `embed_migrations!()` macro for zero-dependency runtime execution on container startup.
- **Baseline Support (Legacy & Squash):**
  - Ability to baseline existing databases: `cargo db-baseline --version 1.0.0` records the current state as a starting point without running historical migrations.
  - Enables squashing large migration histories into a clean foundation.
- **Database Introspection Engine:**
  - Direct introspection of `pg_catalog` and `information_schema`.
  - Captures tables, views, schemas, primary/foreign/unique keys, nullability, defaults, enums, arrays, JSONB, and UUIDs.
  - Generates zero-sized column and table markers, strongly typed `Record` structs, and foreign key relations.
- **Schema Snapshot (`schema.lock`) & Deterministic Drift Detection:**
  - Emits a normalized canonical schema snapshot (`schema.lock`) and embeds a compile-time checksum (`pub const SCHEMA_HASH: &str = "...";`) inside generated Rust schema code.
  - **CI Drift Verification (`cargo db-verify`):** runs migrations on an ephemeral test database and verifies that the live database matches `schema.lock` and `SCHEMA_HASH` bit-for-bit.
  - Fail-fast error reporting when code and database diverge:
    ```text
    Error: Schema drift detected!
    Column `users.phone_number` exists in migrations but is missing from `schema.rs`.
    Hint: Run `cargo db-codegen` to update code and snapshot.
    ```

### Example Usage
```bash
# 1. Apply pending migrations
cargo db-migrate up

# 2. Introspect database, generate Rust schema, and create schema.lock
cargo db-codegen --database-url postgres://localhost:5432/mydb --output src/db/schema.rs

# 3. Verify in CI that migrations, live DB, and generated Rust code are 100% in sync
cargo db-verify --database-url postgres://localhost:5432/testdb
```

---

## Phase 6: Production Hardening: Dual Engine, Multi-Dialect (SQLite) & Pooling

### Objective
Harden the library into an enterprise-grade SQL toolkit suitable for high-throughput production services, embedded use cases, and flexible deployment environments.

### Key Deliverables
- **First-Class SQLite Dialect & Driver:**
  - `Sqlite` dialect implementation (`?` positional parameter placeholders, SQLite identifier quoting).
  - Driver adapter for `rusqlite` (synchronous) and optional async wrappers.
  - Compile-time capability matrix verification ensuring incompatible Postgres constructs cannot be compiled against SQLite.
  - SQLite schema introspection support in the codegen CLI (`sqlite_master` / `PRAGMA table_info`).
- **Dual Engine Architecture:**
  - Asynchronous driver (`tokio-postgres`).
  - Synchronous driver (`postgres`) for CLI tools, workers, and non-tokio runtimes.
- **Transaction Management:**
  - RAII transactions with automatic rollback on error:
    `db.transaction(|tx| async move { ... })`.
  - Configurable transaction isolation levels across supported engines.
- **Connection Pooling:**
  - First-class support for connection pools (`deadpool-postgres`, `bb8`, `r2d2`).
- **Escape Hatches & Raw SQL Interop:**
  - Type-safe raw SQL fragments (`sql!("custom_func({})", users::ID)`).
  - Window functions (`OVER (PARTITION BY ... ORDER BY ...)`).
