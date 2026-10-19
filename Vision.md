# RURQE: Product Vision & Architectural Specification

**RURQE** (**R**ust **U**niversal **R**elational **Q**uery **E**ngine) is an ergonomic, strictly type-safe SQL query builder, relational execution engine, and database-first codegen toolkit for Rust.

---

## 1. Core Vision: The Foundational Tetrad

RURQE synthesizes four foundational superpowers into a unified, uncompromising developer experience:

```text
┌─────────────────────────────────────────────────────────────────────────────────────────┐
│                                 THE FOUNDATIONAL TETRAD                                 │
├───────────────────────┬────────────────────────┬───────────────────────┬────────────────┤
│   Kysely Ergonomics   │  jOOQ Expressiveness   │   Flyway Lifecycle    │Rust Type Safety│
│         (DX)          │   & Relational Depth   │       Integrity       │  & Zero-Cost   │
├───────────────────────┼────────────────────────┼───────────────────────┼────────────────┤
│ • Fluent syntax       │ • Database-first truth │ • Versioned (V) &     │• Compile guards│
│ • Named projections   │ • Complex joins & CTEs │   Repeatable (R)      │• Zero cost     │
│ • Zero boilerplate    │ • Sound nullability    │ • SHA-256 checksums   │• Clear errors  │
│ • First-class tooling │ • Catalog introspection│ • Baseline & Repair   │• Version gates │
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
- **Stage as the Primary Unit of Evolution:** Each stage provides a complete, declarative SQL schema definition across one or multiple files. Any stage can initialize an empty database independently without executing historical migration chains.
- **Verifiable Transitions:** Explicit transitions connect stages ($A \to B$), combining DDL changes and data transformations. Transitions are mathematically verifiable in CI: $\text{State}(A) + \text{Transition}(A \to B) \equiv \text{State}(B)$.
- **Current Schema as Single Source of Truth:** Developers and LLMs interact exclusively with the declarative schema of the current stage. Understanding the database never requires mentally reconstructing fragmented migration histories.
- **Optional History Retention:** Migration history is an operational deployment detail retained only while active environments require it. Obsolete stages and transitions can be pruned cleanly without baseline reconstruction or squashing pain.
- **Flyway Lifecycle Compatibility:** Proven Flyway-grade standards (versioned `V` and repeatable `R` scripts, SHA-256 checksum audits, baseline and repair workflows) are retained as execution foundations and legacy bridges.

### 1.4 Rust Strict Type Safety & Zero-Cost Guarantees
- **Zero-Cost Abstractions:** Monomorphized query builders and phantom markers compile down to static SQL buffers and direct deserializers with zero virtual calls or reflection overhead.
- **Compile-Time Capability & Version Gating:** Dialect- and version-dependent SQL features (such as `RETURNING`, `MERGE`, or `DISTINCT ON`) are enforced via capability marker traits; incompatible queries simply fail to compile.
- **Human- and LLM-Friendly Diagnostics:** Explicit diagnostics via `#[diagnostic::on_unimplemented]` and fail-fast call-site checking ensure short, actionable compiler error messages rather than multi-clause trait cascade dumps.

### 1.5 Core Design Requirements

#### 1.5.1 Absolute SQL Correctness
Given an accurate database schema, every query accepted by RURQE must be semantically valid for the target database dialect and version.
- **Soundness:** Invalid queries must be rejected at compile time.
- **Completeness:** Every valid SQL query must be expressible.
- **Semantic Diagnostics:** Errors must clearly explain the violated SQL rule, not expose Rust type-system internals.

#### 1.5.2 High-Level SQL DSL
The primary public API must express SQL concepts, not Rust type-system machinery.
- **Automatic Inference:** Automatically infer types, scopes, nullability, and result structures.
- **Internal Complexity Encapsulation:** Hide marker types, traits, generics, and typestate complexity from everyday application queries.
- **Zero Machinery Leakage:** Require no knowledge of internal implementation mechanisms for regular query authoring.
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
1. **Stage-Based Evolution Architecture:** Stages define independently reproducible, verifiable schema states. Upgrades follow verified transition chains recorded in the `__rurqe_stage_history` directed acyclic graph.
2. **AI-First Declarative Schema Context:** The current stage DDL serves as the single source of truth for both developers and LLMs, eliminating multi-migration context pollution and token waste.
3. **Continuous Golden Verification (`cargo db-verify`):** Ephemeral CI runs verify that $\text{State}(A) + \text{Transition}(A \to B)$ matches $\text{State}(B)$ bit-for-bit against `schema.lock` and catalog snapshots.
4. **Zero-Pain History Pruning:** Safe removal of obsolete stages when no deployed database requires them, with no baseline reconstruction needed.
5. **Flyway-Grade Migration Support:** Full support for versioned migrations (`V...`), repeatable migrations (`R...`), deterministic ordering, SHA-256 integrity, and repair workflows for traditional setups.

---

## 4. Benchmark Comparison

| Feature / Property | RURQE | jOOQ (Java) | Kysely (TypeScript) | Flyway (Java/CLI) | Diesel / SQLx (Rust) |
|---|---|---|---|---|---|
| **Dialect Capability Gates** | Compile-time (Trait markers) | Runtime check / exception | Runtime syntax error | N/A (DDL-only) | Mostly unchecked / dialect-tied |
| **Null-Safety on Outer Joins** | Strict `Option<T>` compile-time lifting | Nullable at runtime (`NullPointerException` risk) | Optional in types, prone to `as` escapes | N/A (DDL-only) | Varies (complex macros or manual) |
| **Ad-Hoc Named Projections** | Zero-boilerplate via `select!` | Record classes / ad-hoc tuples | Native object literals | N/A (DDL-only) | Tuples or manual DTO structs |
| **Single Source of Truth** | DB-first codegen + `schema.lock` | DB-first codegen | DB-first types generator | Schema history table | Diesel: CLI / SQLx: offline JSON |
| **Migration & Schema Evolution** | AI-First Stages + Flyway (Verifiable $A \to B$, optional history) | External integration (Flyway/Liquibase) | External plugins / basic runners | Gold standard (V + R, repair, baseline) | Basic sequential up/down |
| **Error Message Ergonomics** | Tailored via `#[diagnostic]` & fail-fast | Clear compiler / runtime errors | TypeScript diagnostics | Clear CLI / log errors | Verbose multi-clause trait dumps |
| **Runtime Overhead** | Zero-cost (no GC, monomorphized) | Moderate (GC allocations, reflection) | Moderate (V8 JS heap, string ops) | JVM startup / CLI process | Minimal (zero-cost) |
