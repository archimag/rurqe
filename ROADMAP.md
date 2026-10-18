# Project Roadmap: Type-Safe SQL Query Builder & Codegen for Rust

This document outlines the phased roadmap for building an ergonomic, strictly type-safe SQL query builder and codegen library for Rust, combining the architectural principles of **jOOQ** (database-first codegen, strong typing, relational soundness) and **Kysely** (ergonomic typed queries, named ad-hoc selections, minimal boilerplate).

The roadmap is structured into vertical slices: **at the end of each phase, the library is fully functional, cohesive, and immediately useful**.

---

## Evolution Overview

| Phase | Focus | User Capability | Product Maturity |
|---|---|---|---|
| **Phase 1** | Type-Safe Core & AST | Strict `SELECT` with tuple mapping (Postgres first, dialect-ready) | Minimal Viable Query Builder |
| **Phase 2** | Kysely DX & Macros | Named fields via `select!`, DTO derivation | High-Ergonomics Read Layer |
| **Phase 3** | Type-Safe DML | `INSERT`, `UPDATE`, `DELETE`, `RETURNING` | Complete CRUD Solution |
| **Phase 4** | Relational Power | Joins, compile-time nullability, group by | Advanced Relational Builder |
| **Phase 5** | jOOQ-Style Codegen | CLI database introspection (`pg_catalog`) | Zero-Boilerplate Type System |
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
- `SupportsOnConflict: Dialect` (PostgreSQL, SQLite)

Methods requiring specific SQL features are gated via trait bounds:
```rust
impl<D: SupportsDistinctOn, T: Table> SelectQuery<D, T> {
    pub fn distinct_on<C: Column>(self, col: C) -> Self { ... }
}
```
Attempting to invoke `.distinct_on(...)` on a `SelectQuery<Sqlite, ...>` produces a compile error: *the trait `SupportsDistinctOn` is not implemented for `Sqlite`*.

### 3. Dialect-Specific Extension Traits
Engine-exclusive syntax (such as PostgreSQL JSONB operators `@>`, `?|`, regex matching `~`, or full-text search) is exposed via dedicated extension traits (`PgExpressions`, `SqliteExpressions`) that decorate expressions only when the active dialect matches.

---

## Phase 1: Type-Safe Core & Tuple Select

### Objective
Create a lightweight, strictly typed query builder core with an extensible multi-dialect architecture, capable of constructing parameterized SQL queries and decoding rows into typed tuples without runtime overhead.

### Key Deliverables
- **Multi-Dialect Foundation:**
  - `Dialect` trait specifying identifier quoting and placeholder formatting.
  - `Postgres` dialect implementation (`$1, $2, ...` placeholders, double-quote identifiers).
  - Capability marker traits infrastructure (`SupportsReturning`, `SupportsDistinctOn`).
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
- **Dialect-Gated `RETURNING` Clause:**
  - Available where `D: SupportsReturning`.
  - Return individual columns, typed tuples, or complete record structs upon mutation.
- **Dialect-Aware Conflict Handling (Upsert):**
  - Standard `ON CONFLICT (columns) DO NOTHING / DO UPDATE` for dialects implementing `SupportsOnConflict`.
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

## Phase 5: jOOQ-Style Codegen: Database Schema Introspection CLI

### Objective
Eliminate manual schema maintenance by deriving the complete Rust schema directly from a live PostgreSQL database.

### Key Deliverables
- **Introspection CLI & Build Plugin:**
  - Standalone executable (`cargo-codegen`) and `build.rs` integration.
  - Queries `pg_catalog` and `information_schema` directly.
- **Introspected Schema Features:**
  - Tables, views, and schemas (namespaces).
  - Primary keys, foreign keys, and unique constraints.
  - PostgreSQL data types: primitives, enums, arrays, `timestamptz`, `uuid`, `jsonb`.
  - Nullability and column default expressions.
- **Generated Artifacts:**
  - Zero-sized column and table markers.
  - Full record structs (`Record`, `NewRecord`, `UpdateRecord`).
  - Strongly typed foreign key navigation helpers.

### Example Usage
```bash
cargo db-codegen --database-url postgres://localhost:5432/mydb --output src/db/schema.rs
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
