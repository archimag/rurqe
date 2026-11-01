# RURQE: Product Vision & Architectural Specification

**RURQE** (**R**ust **U**niversal **R**elational **Q**uery **E**ngine) is an ergonomic, strictly type-safe SQL query builder, relational execution engine, and database-first codegen toolkit for Rust.

---

## 1. Core Vision: The Foundational Tetrad

RURQE synthesizes four foundational superpowers into a unified, uncompromising developer experience:

```text
┌─────────────────────────────────────────────────────────────────────────────────────────┐
│                                 THE FOUNDATIONAL TETRAD                                 │
├───────────────────────┬────────────────────────┬───────────────────────┬────────────────┤
│   Kysely Ergonomics   │  jOOQ Expressiveness   │    Atlas Evolution    │Rust Type Safety│
│         (DX)          │   & Relational Depth   │  & Declarative SSOT   │  & Zero-Cost   │
├───────────────────────┼────────────────────────┼───────────────────────┼────────────────┤
│ • Fluent syntax       │ • Database-first truth │ • Declarative schema/ │• Compile guards│
│ • Named projections   │ • Complex joins & CTEs │ • Active delta tail   │• Zero cost     │
│ • Zero boilerplate    │ • Sound nullability    │ • Ephemeral dev DB    │• Clear errors  │
│ • First-class tooling │ • Catalog introspection│ • Golden verification │• Version gates │
└───────────────────────┴────────────────────────┴───────────────────────┴────────────────┘
```

### 1.1 Kysely Ergonomics (Developer Experience)
- **Named Ad-Hoc Projections:** Ergonomic projection macros (`select!`) synthesize strongly typed row records with named fields (`user.id`, `user.email`), eliminating tuple indexing (`row.0`, `row.1`) without requiring manual DTO boilerplate.
- **Zero-Boilerplate Fluent API:** Queries read like idiomatic, clean SQL with minimal syntactic noise and no generic parameter explosion.
- **First-Class Tooling:** Instant autocompletion, inlay hints, and rename-refactoring support in `rust-analyzer` without requiring a running database server during compilation.

### 1.2 jOOQ Expressiveness & Relational Soundness
- **Database-First Single Source of Truth:** Schema metadata is introspected directly from the live database catalog, eliminating duplicated hand-written schema definitions and drift.
- **Relational Expressiveness:** First-class support for advanced SQL capabilities: multi-table joins, subqueries, CTEs (`WITH`), window functions, aggregations, and `CASE WHEN` constructs.
- **Relational Type Invariants:** Mathematically sound compile-time nullability tracking (e.g. automatically lifting non-nullable columns to `Option<T>` in `LEFT JOIN` and outer join operations).

### 1.3 Stage-Based Evolution & Lifecycle Integrity (AI-First Database Management)
- **Atlas-Grade Declarative Evolution with Industrial Safety:** Unifies the modern declarative migration authoring paradigm pioneered by Atlas with zero-compromise backward compatibility for classic Flyway-style sequential migrations.
- **Current Stage as Single Source of Truth (`schema/`):** The repository maintains a single, clean declarative DDL state of the target schema (splittable across domain files like `users.sql`, `orders.sql`). Developers and LLMs interact exclusively with this state without reading historical migration trails.
- **Active Migration Tail (`migrations/`):** Sequential delta scripts (`V...`, `R...`) transform live existing production databases from an established checkpoint into the current stage, handling both DDL and DML data migrations.
- **Cold-Start Instant Initialization:** Clean environments (CI, local dev, container spin-up) bypass historical migrations entirely, initializing the database directly from `schema/` in milliseconds with state markers automatically recorded.
- **Checkpoint Markers & Zero-Pain Pruning:** Cryptographic checkpoint markers (`checkpoint.toml`) anchor active migrations to a base stage hash. Historical migration files older than the checkpoint can be pruned freely (`rm V001..V050`) without squashing or baseline invalidation.
- **The Golden Equation Verification:** Continuous verification ensures mathematical parity: $\text{State}(\text{schema/}) \equiv \text{State}(\text{Checkpoint} + \text{migrations/})$. Divergence is caught in CI before merging.
- **Fail-Safe Outdated Target Protection:** The migration runner aborts with an actionable diagnostic if an existing target database is at a version older than the pruned checkpoint boundary.
- **Pragmatic SQLite Migration Concurrency:** SQLite serialises writers through database-file transactions (`BEGIN IMMEDIATE`). Because non-transactional operations (`VACUUM`, `PRAGMA`) are rejected inside transactions, the lock is committed during autocommit execution. Synthetic external lockfiles across autocommit statements are explicitly rejected as YAGNI for embedded/low-concurrency SQLite deployments.

### 1.4 Rust Strict Type Safety & Zero-Cost Guarantees
- **Zero-Cost Abstractions:** Monomorphized query builders and phantom markers compile down to static SQL buffers and direct deserializers with zero virtual calls or reflection overhead.
- **Compile-Time Capability & Version Gating:** Dialect- and version-dependent SQL features (such as `RETURNING`, `MERGE`, or `DISTINCT ON`) are enforced via capability marker traits; incompatible queries simply fail to compile.
- **Human- and LLM-Friendly Diagnostics:** Explicit diagnostics via `#[diagnostic::on_unimplemented]` and fail-fast call-site checking ensure short, actionable compiler error messages rather than multi-clause trait cascade dumps.

### 1.5 Core Design Requirements

#### 1.5.1 Absolute SQL Correctness & Two-Tier Guarantee
Given an accurate database schema, every query accepted and built by RURQE must be semantically valid for the target database dialect and version, guaranteeing **Zero Database Rejection**.
- **Two-Tier Soundness Architecture:**
  - **Tier 1 (Compile-Time Type Soundness):** All core SQL relational invariants—including relational scope isolation, projection types, outer-join nullability lifting, subquery arity and type congruence in `IN` predicates, grouping coverage on `GroupedQuery` (restricting projections, `HAVING`, and window clauses to grouped or aggregate expressions), and dialect capability gating—are enforced strictly at compile time on stable Rust.
  - **Tier 2 (Pre-Flight Client-Side Validation in `.build()`):** Complex multi-clause semantic invariants that cannot be soundly expressed in stable Rust's open-world type system without nightly features—including disjoint relation checks in unaliased duplicate `JOIN`s, leading `ORDER BY` prefix alignment in PostgreSQL `DISTINCT ON`, and residual aggregation validation without an explicit `GROUP BY` (rejecting an aggregate projected alongside a bare column via `UngroupedColumnWithAggregate`, as an un-grouped query carries no grouping typestate at the type level)—are validated deterministically during `.build()` before any network dispatch.
  - **Zero Database Rejection:** No syntactically or semantically invalid query reaches the database engine; errors are caught either at compile time or during local AST assembly.
- **Soundness on Strictly Stable Rust:** Invalid queries are rejected using strictly stable Rust compiler capabilities (Rust MSRV). Nightly-only compiler flags or features (`#![feature(...)]`, `negative_impls`, `generic_const_exprs`) are strictly prohibited.
- **Completeness:** Every valid SQL query must be expressible.
- **Semantic Diagnostics:** Errors must clearly explain the violated SQL rule, not expose Rust type-system internals.

#### 1.5.2 High-Level SQL DSL
The primary public API must express SQL concepts, not Rust type-system machinery.
- **Automatic Inference:** Automatically infer types, scopes, nullability, and result structures.
- **Internal Complexity Encapsulation:** Hide marker types, traits, generics, and typestate complexity from everyday application queries.
- **Zero Machinery Leakage:** Require no knowledge of internal implementation mechanisms for regular query authoring.
- **Pre-Flight Ergonomics:** High-level query composition retains fluent, unencumbered builder chaining while ensuring structural validation rules surface as clear, typed error diagnostics during query building rather than confusing type-solver cascades.
- **Tiered Access:** Preserve a lower-level typed AST and macro escape-hatches (`sql!`, `raw_sql!`) for advanced and dialect-specific use cases.

**Guiding Principle:** Maximum semantic guarantees and SQL expressiveness, minimum cognitive complexity for the developer.

---

## 2. Workspace Crate Architecture

RURQE is designed as a modular Cargo workspace, enabling minimal dependency footprints and fast compilation:

- **`rurqe`**: The primary facade crate providing high-level re-exports and fluent entrypoints.
- **`rurqe-core`**: Pure, lightweight AST representation, schema markers (`Table`, `Column`), dialect traits (`Dialect`), capability marker traits, and expression operator logic.
- **`rurqe-macros`**: Declarative and procedural macros (`select!`, `dto!`, `table!`) for zero-cost projection synthesis and compile-time schema DSLs.
- **`rurqe-postgres`**: PostgreSQL dialect renderer, version markers (`Pg14`, `Pg15`, `Pg16`), and execution adapters for `tokio-postgres` and synchronous `postgres`.
- **`rurqe-sqlite`**: SQLite dialect renderer, version bounds (`Sqlite3_35`, etc.), and driver adapter for `rusqlite`.
- **`rurqe-cli`**: Developer toolchain for live catalog introspection, code generation, migration execution, and CI schema drift verification (`schema.lock`).

---

## 3. Architectural Pillars

### 3.1 Multi-Dialect Architecture & Version Gating
To support multiple SQL engines (PostgreSQL primary, followed by SQLite and MySQL) without runtime overhead or lowest-common-denominator compromises:

1. **Zero-Cost Dialect Trait:** Encapsulates dialect-specific SQL rendering rules (quoting, parameter placeholders `$N` vs `?`, literal escaping). Dialect is a zero-sized phantom type parameter on queries (e.g., `SelectQuery<Postgres, ...>`).
2. **Compile-Time Capability Traits:** Features unsupported by a dialect or version are represented as marker traits (`SupportsDistinctOn`, `SupportsReturning`, `SupportsOnConflict`). Unsupported methods do not exist for incompatible dialects.
3. **Type-Level Engine Versions:** Dialects accept an optional type-level version parameter with modern defaults (e.g., `Postgres<V = Pg16>`, `Sqlite<V = Sqlite3_35>`). Capability traits are conditionally implemented based on version thresholds (`V: SqliteVersionAtLeast<3, 35>`).
4. **Dialect Extension Traits:** Engine-exclusive syntax (such as PostgreSQL JSONB operators `@>`, `?|`, regex matching `~`, or full-text search) is exposed via dedicated extension traits (`PgExpressions`, `SqliteExpressions`).

### 3.2 Compiler Diagnostics & Error Ergonomics (Human & LLM First)
Preventing the cryptic, 50-line compiler errors notorious in existing Rust ORMs:

1. **Actionable Diagnostics via `#[diagnostic::on_unimplemented]`:** Core traits use Rust 1.78+ diagnostic attributes to emit concise, human- and LLM-readable messages with remediation hints.
2. **Fail-Fast Call-Site Validation:** Type mismatches are caught immediately at the offending method call (`.where_()`, `.eq()`, `.select()`) rather than deferred to terminal execution methods (`.fetch_all()`).
3. **Flat Query Types:** Signatures expose only high-level contracts (`Query<Dialect, TablesInScope, OutputRow>`), keeping rust-analyzer hints and compiler output clean without generic parameter cascades.
4. **Semantic Domain Markers:** Trait bounds use clear domain terminology (`ColumnBelongsTo<Table>`, `TableInScope<Table>`, `SqlCompatible<ColType, ValType>`).
5. **Explicit Macro Diagnostics:** Macros produce intentional `compile_error!("...")` feedback with actionable guidance.

### 3.3 Stage-Based Evolution & Deterministic Drift Verification
1. **Unified Stage + Migration Tail Architecture:** Combines a clean declarative target schema (`schema/`) with an active sequence of delta migrations (`migrations/`) anchored by a checkpoint marker.
2. **AI-First Single Source of Truth:** LLMs and developers reason about a single declarative schema snapshot, eliminating token waste and contextual fragmentation from dozens of historical migration scripts.
3. **Continuous Golden Verification (`cargo rurqe verify`):** CI spins up ephemeral databases to verify that $\text{State}(\text{schema/}) \equiv \text{State}(\text{Checkpoint} + \text{migrations/})$ bit-for-bit against `schema.lock` and catalog diffs.
4. **Zero-Pain History Pruning:** Teams safely delete obsolete migrations once all deployed environments have progressed past the checkpoint version, without rewriting baseline checksums.
5. **Fail-Safe Checkpoint Protection:** If a legacy database lags behind the active checkpoint, the migration engine rejects execution with an actionable remediation guide rather than silently corrupting state.
6. **Classic Sequential Compatibility:** Teams preferring traditional sequential migrations continue using versioned (`V...`) and repeatable (`R...`) scripts with SHA-256 audit trails, repair, and baseline facilities.

---

## 4. Benchmark Comparison

| Feature / Property | RURQE | jOOQ (Java) | Kysely (TypeScript) | Atlas / Flyway | Diesel / SQLx (Rust) |
|---|---|---|---|---|---|
| **Dialect Capability Gates** | Compile-time (Trait markers) | Runtime check / exception | Runtime syntax error | N/A (DDL-only) | Mostly unchecked / dialect-tied |
| **Null-Safety on Outer Joins** | Strict `Option<T>` compile-time lifting | Nullable at runtime (`NullPointerException` risk) | Optional in types, prone to `as` escapes | N/A (DDL-only) | Varies (complex macros or manual) |
| **Ad-Hoc Named Projections** | Zero-boilerplate via `select!` | Record classes / ad-hoc tuples | Native object literals | N/A (DDL-only) | Tuples or manual DTO structs |
| **Single Source of Truth** | DB-first codegen + `schema.lock` | DB-first codegen | DB-first types generator | Atlas: HCL/SQL / Flyway: history | Diesel: CLI / SQLx: offline JSON |
| **Migration & Schema Evolution** | AI-First Stage + Tail (Verifiable $A \to B$, safe pruning) | External integration (Flyway/Liquibase) | External plugins / basic runners | Declarative diffing (Atlas) / V+R (Flyway) | Basic sequential up/down |
| **Error Message Ergonomics** | Tailored via `#[diagnostic]` & fail-fast | Clear compiler / runtime errors | TypeScript diagnostics | Clear CLI / log errors | Verbose multi-clause trait dumps |
| **Runtime Overhead** | Zero-cost (no GC, monomorphized) | Moderate (GC allocations, reflection) | Moderate (V8 JS heap, string ops) | External Go/JVM process | Minimal (zero-cost) |
