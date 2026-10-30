# RURQE: Project Roadmap & Evolution Plan

This document details the progressive execution roadmap for **RURQE** (**R**ust **U**niversal **R**elational **Q**uery **E**ngine).

For core vision, design philosophy, architectural foundations, and benchmark comparisons, see [`Vision.md`](Vision.md).

---

## Evolutionary Slices: Overview

The roadmap is structured into vertical slices: **at the end of each phase, the library is fully functional, cohesive, and immediately usable**.

| Phase | Focus | User Capability | Product Maturity |
|---|---|---|---|
| **Phase 1** [x] | Type-Safe Core & AST | Strict `SELECT` with tuple mapping (Postgres first, dialect-ready) | Minimal Viable Query Builder (Completed) |
| **Phase 2** [x] | Kysely DX & Macros | Named fields via `select!`, DTO derivation | High-Ergonomics Read Layer (Completed) |
| **Phase 3** [x] | Type-Safe DML | `INSERT`, `UPDATE`, `DELETE`, `RETURNING`, `UPSERT` | Complete CRUD Solution (Completed) |
| **Phase 4** [x] | Relational Power | Joins, compile-time nullability, `GROUP BY`, aggregations | Advanced Relational Builder (Completed) |
| **Phase 5** [x] | Codegen, Migrations & Drift Verification | CLI introspection, migration engine, baseline & `schema.lock` | Zero-Boilerplate & Verified Integrity (Completed) |
| **Phase 6** [x] | Multi-Dialect & Execution Adapters | SQLite dialect (`Sqlite<V>`), dual engine (`tokio-postgres`/`postgres`/`rusqlite`) | Multi-Engine Execution Layer (Completed) |
| **Phase 7** [x] | Runtime Hardening: Transactions & Pools | RAII transaction scopes, savepoints, connection pools (`deadpool`, `r2d2`) | High-Throughput Production Ready (Completed) |
| **Phase 8** [x] | Advanced SQL & SQLite Toolchain Parity | Window functions, CTEs (`WITH`), SQLite CLI introspection | Full Expressiveness & Multi-Engine Parity (Completed) |
| **Phase 9** [x] | Core SQL Expressiveness & Set Operations | `IS NULL`, `CASE WHEN`, `EXISTS`, `UNION / ALL` | Rich Expressions & Set Algebra (Completed) |
| **Phase 10** [x] | Advanced Relational: Aliasing, Lateral Joins & DML | Table aliasing, self-joins, `LATERAL JOIN`, `INSERT INTO SELECT` | Deep Relational Power (Completed) |
| **Phase 11** [x] | Vision Alignment & Semantic Ergonomics | Actionable diagnostics, fluent predicates, ergonomic aliases, multi-table DML | Reference Ergonomics & Soundness (Completed) |
| **Phase 12** [x] | Two-Tier Architecture & High-Level SQL DSL | Value-based schema, zero-turbofish queries, automatic nullability, universal record synthesis | Two-Tier Architecture & Reference SQL DSL (Completed) |
| **Phase 13** [x] | Codebase Hardening & Conventions Refactoring | Large module decomposition (<800 LOC), panic-free typestate | Production Rigor & Standard Compliance (Completed) |
| **Phase 14** [x] | Rich Types, Custom Domain Types & Observability | Ecosystem types (UUID, JSON, Chrono/Time) via features, custom binary/text newtypes, enums, tracing | Type System Completeness & Observability (Completed) |
| **Phase 15** [x] | Stage-Based Schema Evolution | AI-first declarative schema stages, verifiable transition paths, optional history retention | Stage-Based Schema Evolution & AI-First Lifecycle (Completed) |
| **Phase 16** [x] | Compile-Fail Hardening & Negative Test Suite | Compile-time rejection of every scope, nullability, typestate, capability, and grouping violation | Absolute SQL Correctness (Completed) |
| **Phase 17** [x] | Relational Depth, Row Expressions & Locking | Row expressions `(a, b)`, pessimistic locking (`FOR UPDATE`), scalar subqueries as `Expr<T>`, window frames, `FILTER (WHERE)`, `EXCLUDED` | Relational Algebra & Transactional Soundness (Completed) |
| **Phase 18** [x] | Kysely DX, Dynamic Builders & Nested DTOs | Typed nested records (`nest_many`/`nest_one` via native JSON), fluent conditional builders (`.when`, `.if_some`), derived tables, DB-first enums, PostgreSQL arrays (`T[]`) | High-Ergonomics Relational Power (Completed) |
| **Phase 19** [x] | Expression Polish, Practical DML & Advanced Ordering | Arithmetic `std::ops`, partial upsert `ON CONFLICT WHERE`, `NULLS FIRST/LAST`, `Decimal`, scalar SQL functions, JSONB extraction, `FULL OUTER JOIN` | High-ROI Relational Polish (Completed) |
| **Phase 20** [x] | Absolute SQL Soundness: Scoping, Grouping & Ordering Typestates | Unaliased duplicate join guards, subquery arity/type checks in `IN`, sound function grouping, `DISTINCT`/`DISTINCT ON` & compound `ORDER BY` typestates | Uncompromising SQL Soundness (Completed) |

---

## Phase 1: Type-Safe Core & Tuple Select [Completed]

### Objective
Create a lightweight, strictly typed query builder core with an extensible multi-dialect architecture, capable of constructing parameterized SQL queries and decoding rows into typed tuples without runtime overhead.

### Key Deliverables
- [x] **Multi-Dialect Foundation:**
  - `Dialect` trait specifying identifier quoting and placeholder formatting.
  - `Postgres` dialect implementation (`$1, $2, ...` placeholders, double-quote identifiers, version markers `Pg14`, `Pg15`, `Pg16`).
  - Capability marker traits infrastructure (`SupportsReturning`, `SupportsDistinctOn`, `SupportsOnConflict`).
- [x] **Core Abstractions:**
  - `Table` trait: table identifier and compile-time name.
  - `Column` trait: column name, parent table marker, and associated `RustType`.
  - Type-safe diagnostics via `#[diagnostic::on_unimplemented]` (`ColumnBelongsTo`, `TableInScope`, `SqlCompatible`).
  - `Expr<T>` AST and typed operators (`eq`, `ne`, `gt`, `lt`, `and`, `or`).
- [x] **Query Construction:**
  - `SelectQuery<D: Dialect, ...>` builder supporting `.from()`, `.select()`, `.where_()`, `.order_by()`, `.limit()`, and `.offset()`.
  - Deterministic parameterized SQL renderer driving output via the active `Dialect`.
- [x] **Manual Schema Declaration DSL:**
  - A lightweight declarative macro (`table!`) for defining table and column markers in under 10 lines of code per table.
- [x] **Row Decoding:**
  - Zero-overhead row extraction traits (`FromRow`, `FromSqlValue`) mapping SQL values into typed tuples for arities 1..=12.

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

## Phase 2: Kysely DX: Macros for Named Projections (`select!` & DTOs) [Completed]

### Objective
Eliminate tuple indexing (`row.0`, `row.1`) in favor of named fields matching Kysely's ergonomic developer experience, using Rust's macro system without requiring compile-time database connectivity.

### Key Deliverables
- [x] **Ad-Hoc Projection Macro (`select!`):**
  - Synthesizes an anonymous local row struct at the call site.
  - Resolves field types automatically from associated column markers (`<Col as Column>::RustType`).
  - Supports field aliasing (`users::EMAIL as contact_email`).
  - Implements row decoding traits automatically for the generated struct.
- [x] **DTO Macro (`dto!`):**
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

## Phase 3: Type-Safe DML (Insert, Update, Delete, Returning, Upsert) [Completed]

### Objective
Expand the query builder into a comprehensive CRUD library with compile-time safety guards against common runtime mistakes.

### Key Deliverables
- [x] **Type-Safe `INSERT`:**
  - Strongly typed column assignment (`UserId::set(...)`).
  - Support for `DEFAULT` values (e.g. autoincrement primary keys).
  - Bulk inserts (`values_many`).
- [x] **Compile-Time Safe `UPDATE` & `DELETE`:**
  - Typestate enforcement: prevents executing `UPDATE` or `DELETE` without a `WHERE` clause unless explicitly overridden (`.allow_all()`).
- [x] **Dialect- and Version-Gated `RETURNING` Clause:**
  - Available where `D: SupportsReturning` (PostgreSQL, SQLite 3.35+).
  - Return individual columns, typed tuples, or complete record structs upon mutation.
- [x] **Dialect- and Version-Aware Conflict Handling (Upsert):**
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

## Phase 4: Relational Querying: Joins, Nullability Tracking & Aggregations [Completed]

### Objective
Provide advanced relational querying capabilities comparable to jOOQ, maintaining sound compile-time nullability tracking across joins.

### Key Deliverables
- [x] **Join Semantics:**
  - `INNER JOIN` and `LEFT JOIN` with typed `ON` predicates.
- [x] **Automatic Sound Nullability Tracking:**
  - Type-level transformation: columns participating in a `LEFT JOIN` automatically wrap their Rust type in `Option<T>` at compile time via `Nullable<Col>`.
- [x] **Aggregations & Grouping:**
  - Standard aggregate functions (`count_all`, `count`, `sum`, `avg`, `min`, `max`).
  - `.group_by(...)` and `.having(...)` validation and parameter binding.
- [x] **Conditional Expressions:**
  - Type-safe `Expr::<T>::case().when(condition, value).else_(fallback).end()` producing `CASE WHEN ... THEN ... ELSE ... END` (delivered in Phase 9).

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

## Phase 6: Multi-Dialect & Execution Adapters [Completed]

### Objective
Provide first-class SQLite dialect support, version threshold capability gating, and synchronous and asynchronous execution adapters across SQLite and PostgreSQL.

### Key Deliverables
- [x] **First-Class SQLite Dialect & Driver:**
  - `Sqlite<V>` dialect implementation (`?` positional parameter placeholders, SQLite identifier quoting).
  - Driver adapter for `rusqlite` (synchronous `SqliteConnection`), implementing the shared `QueryBackend` execution contract.
  - Compile-time capability matrix verification: `Sqlite3_24` gates `ON CONFLICT`, `Sqlite3_35` gates `RETURNING`.
- [x] **Dual Engine Architecture:**
  - Asynchronous driver (`tokio-postgres`) through `PgAsyncConnection` with mirrored `async` helpers.
  - Synchronous driver (`postgres`) for CLI tools, workers, and non-tokio runtimes through `PgConnection`.
  - Unified execution helpers (`fetch_all`, `fetch_one`, `fetch_optional`, `execute`) exported via `rurqe::prelude`.
  - End-to-end CRUD verification suite on in-memory SQLite (`crates/rurqe/tests/sqlite_e2e.rs`).

### Example Usage
```rust
use rurqe::prelude::*;
use rurqe::sqlite::rusqlite::Connection;

let mut db = SqliteConnection::new(Connection::open_in_memory()?);
let users = select_from::<Sqlite<Sqlite3_35>, User>()
    .select((UserId, UserName))
    .fetch_all(&mut db)?;
```

---

## Phase 7: Runtime Hardening: Transactions & Connection Pools [Completed]

### Objective
Provide ergonomic RAII-managed transactions with automatic rollback on error, savepoint-based nesting, and seamless integration with production connection pools (`deadpool-postgres`, `r2d2`).

### Key Deliverables
- [x] **RAII Transaction Scopes:**
  - Synchronous `db.transaction(|tx| { ... })` and asynchronous `db.transaction(|tx| async move { ... })`.
  - Guaranteed `ROLLBACK` on error or panic; atomic `COMMIT` only on `Ok(_)`.
  - Nested transaction scopes backed by SQL `SAVEPOINT` / `RELEASE SAVEPOINT` / `ROLLBACK TO SAVEPOINT`.
- [x] **Transaction Isolation Levels:**
  - Configurable transaction options: `ReadCommitted`, `RepeatableRead`, `Serializable`.
- [x] **Connection Pool Adapters:**
  - Asynchronous pool support via `deadpool-postgres`: pool and pooled client implement execution contracts directly.
  - Synchronous pool support via `r2d2`: pooled connections for `SqliteConnection` and `PgConnection`.
- [x] **Facade Integration & Verification:**
  - Comprehensive transactional integration tests verifying rollback on error, commit persistence, and nested savepoint isolation.

### Example Usage
```rust
use rurqe::prelude::*;

// RAII transaction with automatic rollback on error or panic
let result = db.transaction(|tx| {
    insert_into::<Sqlite<Sqlite3_35>, User>()
        .values((UserId::set(1i64), UserName::set("alice")))
        .execute(tx)?;

    // Nested savepoint transaction
    tx.transaction(|nested| {
        insert_into::<Sqlite<Sqlite3_35>, User>()
            .values((UserId::set(2i64), UserName::set("bob")))
            .execute(nested)?;
        Ok(())
    })?;

    Ok(())
});
```

---

## Phase 8: Advanced SQL Expressiveness & SQLite Toolchain Parity [Completed]

### Objective
Broaden SQL relational expressiveness with window functions, Common Table Expressions (CTEs), type-safe raw SQL escape hatches, and complete SQLite CLI introspection parity.

### Key Deliverables
- [x] **SQLite CLI Introspection & Drift Parity:**
  - Full catalog introspection for SQLite (`sqlite_master`, `PRAGMA table_info`, `PRAGMA foreign_key_list`).
  - Unified `cargo db-codegen` and `cargo db-verify` across both PostgreSQL and SQLite URLs.
- [x] **Window Functions:**
  - `OVER (PARTITION BY ... ORDER BY ...)` clause builder (`Window`, `WindowSpec`).
  - Typed window operators: `row_number()`, `rank()`, `dense_rank()`, `lag()`, `lead()`.
  - Type-safe integration into `.select(...)` projections preserving `FromRow` tuple decoding.
- [x] **Common Table Expressions (CTEs):**
  - Type-safe `WITH cte AS (...) SELECT ...` query composition (`cte`, `cte_named`).
  - Sequential parameter binding and integration with `TableScope` joins.
- [x] **Safe Raw SQL Escape Hatches:**
  - `sql!(...)` and `raw_sql!(...)` procedural macros with bound `SqlVal` parameter interpolation and compile-time placeholder arity validation.

### Example Usage
```rust
use rurqe::prelude::*;

// 1. Analytical window functions with OVER clause
let ranked_query = select_from::<Postgres, Employee>()
    .select((
        EmployeeId,
        row_number().over(Window::new().partition_by(DepartmentId).order_by_desc(Salary)),
    ));

// 2. Common Table Expressions (WITH cte AS ...)
let active_cte = select_from::<Postgres, User>()
    .select((UserId, UserName))
    .where_(IsActive.eq(true))
    .as_cte();

let results = select_from::<Postgres, User>()
    .with(cte(active_cte))
    .select((UserId, UserName));

// 3. Safe raw SQL escape hatch with compile-time parameter binding
let custom_filter = sql!(bool, "EXTRACT(YEAR FROM created_at) = ?", 2024i64);
```

---

## Phase 9: Core SQL Expressiveness & Set Operations (Expressions, Predicates & Sets) [Completed]

### Objective
Eliminate the need for raw SQL escape hatches in common query patterns by introducing first-class null predicates, conditional branching (`CASE WHEN`), standard functions, subquery predicates (`EXISTS`, `IN`), and strictly typed set operations (`UNION`, `INTERSECT`, `EXCEPT`).

### Key Deliverables
- [x] **Null Testing Predicates:**
  - Typed `.is_null()` and `.is_not_null()` operators on columns and expressions.
- [x] **Conditional Branching & SQL Functions:**
  - Fluent `case().when(condition, then_expr).else_(else_expr).end()` builder producing typed `Expr<T>`.
- [x] **Subquery Predicates:**
  - `exists(subquery)` and `not_exists(subquery)`.
  - `column.in_subquery(subquery)` and `column.not_in_subquery(subquery)`.
  - Deferred subquery rendering maintaining unified parameter numbering across parent and nested scopes.
- [x] **Type-Safe Set Operations:**
  - `.union(other_query)` and `.union_all(other_query)`.
  - `.intersect(other_query)` and `.except(other_query)`.
  - Compile-time guarantee: both queries must share identical `Dialect` and output row types (`Output`), preserving `FromRow` decoding and outer trailing ordering/pagination.

### Example Usage
```rust
use rurqe::prelude::*;

// 1. CASE WHEN and COALESCE
let query = select_from::<Postgres, Employee>()
    .select((
        EmployeeId,
        case()
            .when(Salary.gt(100_000i64), "senior")
            .else_("standard"),
        coalesce((Bonus, 0i64)),
    ))
    .where_(DepartmentId.is_not_null());

// 2. Subquery predicates
let active_buyers = select_from::<Postgres, User>()
    .select((UserId, UserName))
    .where_(exists(
        select_from::<Postgres, Order>()
            .where_(OrderUserId.eq(UserId))
    ));

// 3. Type-safe UNION ALL
let all_contacts = query_customers.union_all(query_suppliers);
```

---

## Phase 10: Advanced Relational Composition: Aliasing, Lateral Joins & Advanced DML [Completed]

### Objective
Provide deep relational composition capabilities: table and subquery aliasing for unambiguous self-joins, compile-time capability-gated `LATERAL JOIN`s with sound nullability tracking, and advanced DML (`INSERT INTO ... SELECT`).

### Key Deliverables
- [x] **Table & Subquery Aliasing:**
  - Zero-cost table aliases via `table_alias!`, `Aliased<T, A>`, `AliasedCol<A, C>` and `aliased::<T, A>()`, rendering SQL `\"table\" AS \"alias\"` and addressing columns as `\"alias\".\"column\"`.
  - Unambiguous self-joins: qualifying columns via their alias marker, fully participating in `TableScope`, `ExprScope` and projection macros.
- [x] **First-Class `LATERAL JOIN` Support:**
  - Capability marker trait `SupportsLateralJoin` implemented for PostgreSQL.
  - Correlated subqueries via `.cross_join_lateral(lateral(subquery))` and `.left_join_lateral(lateral(subquery))`.
  - Sound compile-time nullability: `.left_join_lateral(...)` automatically lifts lateral subquery projections into `Option<T>`.
- [x] **Advanced DML:**
  - Type-safe `INSERT INTO ... SELECT`: inserting results of a compatible `SelectQuery` directly into a target table via `.columns(...).from_select(...)` without client round-trips, supporting parameter chaining and `RETURNING`.

### Example Usage
```rust
use rurqe::prelude::*;

// 1. Self-join with table aliases
table_alias!(Mgr, "m");
table_alias!(Emp, "e");

let managers = select_from::<Postgres, Aliased<User, Mgr>>()
    .inner_join(
        aliased::<User, Emp>(),
        AliasedCol::<Mgr, UserId>::new().eq(AliasedCol::<Emp, ManagerId>::new()),
    )
    .select((
        AliasedCol::<Mgr, UserName>::new(),
        AliasedCol::<Emp, UserName>::new(),
    ));

// 2. LATERAL JOIN with capability gating
let latest_orders = select_from::<Postgres, Order>()
    .where_(OrderUserId.eq(UserId))
    .order_by_desc(OrderCreatedAt)
    .limit(1)
    .as_lateral::<LatestOrder>();

let report = select_from::<Postgres, User>()
    .left_join_lateral(latest_orders)
    .select((UserId, OrderTotal.nullable()));

// 3. Type-safe INSERT INTO ... SELECT
let archived = insert_into::<Postgres, ArchiveUser>()
    .columns((ArchiveUserId, ArchiveEmail))
    .from_select(
        select_from::<Postgres, User>()
            .select((UserId, UserEmail))
            .where_(IsActive.eq(false)),
    );
```

---

## Phase 11: Vision Alignment & Semantic Ergonomics [Completed]

### Objective
Align all RURQE APIs and compiler behaviors with the core Vision requirements (Absolute SQL Correctness & High-Level SQL DSL): replace cryptic Rust trait errors with domain-level SQL diagnostics via `#[diagnostic::on_unimplemented]`, introduce zero-friction ergonomic addressing for table aliases and fluent predicates, and support multi-table mutations (`UPDATE ... FROM`, `DELETE ... USING`).

### Key Deliverables
- [x] **Semantic Compiler Diagnostics (Human & LLM First):**
  - Explicit typestate diagnostic on unfiltered mutations (`UpdateQuery`/`DeleteQuery<..., Unfiltered>`) explaining the need for `.where_()` or `.allow_all()` via `ExecutableMutation`.
  - Actionable diagnostics for missing tables in `FROM`/`JOIN` (`SelectionInScope`, `TableInScope`) and foreign columns in `RETURNING` (`TableSelection`).
  - Clear capability mismatch diagnostics for version-gated features (`SupportsReturning`, `SupportsOnConflict`, `SupportsLateralJoin`).
- [x] **High-Level Fluent Predicates:**
  - Inherent methods on columns and expressions: `.like(pat)`, `.ilike(pat)`, `.between(low, high)`, `.in_list(values)`, `.not_in_list(values)`.
- [x] **Ergonomic Aliasing DSL:**
  - Lightweight column and table aliasing syntax (`col.alias::<A>()`, `A.col(Col)`), eliminating repetitive turbofish wrappers (`AliasedCol::<A, C>::new()`).
- [x] **Multi-Table DML Mutations:**
  - Strictly typed `UPDATE ... SET ... FROM ...` and `DELETE ... USING ...` with compile-time join scope checks via `ExtendScope`.
- [x] **Verification & LLM Documentation:**
  - Negative compile-fail doctests asserting actionable error messages.
  - Complete update of `llms.txt` reflecting the pure, unencumbered SQL DSL.

---

## Phase 12: Two-Tier Architecture & High-Level SQL DSL [Completed]

### Objective
Establish a clean two-tier architecture separating the internal monomorphized relational IR from the primary public SQL DSL. Eliminate type-system machinery leaks (turbofish on table/column markers, manual `.nullable()` lifting, tuple projections, repeated dialect generics) in favor of value-based schema arguments (`.from(users)`), dialect-scoped modules (`rurqe::postgres::*`), connection-bound query factories, automatic nullability inference from relation provenance, universal named record synthesis in `select!`, and compile-time `GROUP BY` soundness.

### Key Deliverables
- [x] **Zero-Turbofish Value-Based DSL:**
  - Query builder methods accept zero-sized table values as arguments: `.from(users)` / `.from(Users)`, `.inner_join(orders, ...)` / `.left_join(orders, ...)`.
  - Type-safe DML entrypoints taking table arguments: `insert_into(users)`, `update(users)`, `delete_from(users)` without turbofish syntax.
  - Zero runtime overhead: ZST arguments compile away completely while allowing `rustc` to infer table types immediately.
- [x] **Dialect Ergonomics & Connection Integration:**
  - Specialized dialect modules (`rurqe::postgres::prelude::*`, `rurqe::sqlite::prelude::*`) pre-binding the dialect so application queries never require `<Postgres>` annotations.
  - Turbofish-free named selection start: `select!(...).from(table)` resolves dialect automatically from the active dialect prelude without explicit type parameters.
  - First-class dialect version submodules with dedicated preludes (`rurqe::postgres::pg14::prelude::*`, `rurqe::sqlite::sqlite3_24::prelude::*`) binding specific engine versions and automatically enforcing compile-time capability gates (`SupportsReturning`, `SupportsOnConflict`, etc.) with zero turbofish noise.
  - Connection and transaction query methods (`conn.select_from(users)`, `conn.update(users)`) inferring dialect directly from the database handle.
- [x] **Value-Based Schema & Unified Expression Model:**
  - Codegen and `table!` generate schema objects/values (`users.id`, `orders.total`) that directly implement `Expression<T>`, eliminating `Expr::column::<Col>()` turbofish calls.
  - Fluent operator and method calls directly on columns (`users.age.gt(18)`, `users.name.like("%admin%")`).
  - Value-based table aliasing (`let u1 = users.as_alias("u1");`) without raw `AliasedCol<A, C>` wrappers.
- [x] **Automatic Nullability Inference on Outer Joins:**
  - Outer joins mark relation scopes (`NullableRelation<T>`); columns projected from outer joins automatically lift their types to `Option<T>` without explicit `.nullable::<Col>()` calls.
- [x] **Universal Named Record Projections:**
  - Macro `select!` synthesizes strongly typed ad-hoc structs for any projection items (columns, computed expressions, aggregates, aliased fields), making named field access (`row.id`, `row.total`) universal and phasing out manual tuple indexing.
- [x] **Systematic SQL Soundness (GROUP BY Validation):**
  - Typestate transition to `GroupedQuery` upon `.group_by(...)`, statically restricting projected fields to grouped columns or aggregate expressions, preventing runtime SQL grouping errors at compile time.
- [x] **Domain-Specific Diagnostics & Error Reporting:**
  - Semantic compiler messages in pure SQL terminology, pinpointing out-of-scope tables and grouping violations without exposing inductive position markers (`Next<P>`, `At<0>`).

### Example Usage
```rust
use rurqe::postgres::prelude::*;

// 1. Pure SQL syntax: no turbofish, no <Postgres> generics, named projections
let report = select! {
    user_id: users.id,
    user_name: users.name,
    order_count: count(orders.id),
    total_spent: coalesce(sum(orders.total), 0.0),
}
.from(users)
.left_join(orders, orders.user_id.eq(users.id))
.where_(users.age.gt(18).and(users.email.like("%@company.com")))
.group_by((users.id, users.name))
.having(count(orders.id).gt(0))
.order_by_desc(count(orders.id))
.fetch_all(&mut conn)?;

for r in report {
    println!("User {} (id: {}) spent {}", r.user_name, r.user_id, r.total_spent);
}

// 2. Zero-turbofish DML with compile-time safety
update(users)
    .set(users.name.set("Alice"))
    .where_(users.id.eq(42i64))
    .execute(&mut conn)?;
```

---

## Phase 13: Codebase Hardening, Architecture Refactoring & Convention Compliance [Completed]

### Objective
Bring the entire workspace into strict alignment with [`Conventions.md`](Conventions.md): decompose large files exceeding the 800-line limit into focused submodules (using modern Rust 2018 file hierarchies without `mod.rs`), eliminate runtime panics in production code via Typestate guards and explicit `Result` handling, and audit parsing paths against unhandled panics.

### Key Deliverables
- [x] **Large Module Decomposition (Hard limit < 800 LOC, target 150–400 LOC):**
  - Decomposed `crates/rurqe-core/src/query.rs` (~3770 LOC) into modular facade and submodules (`select.rs`, `selection.rs`, `scope.rs`, `group_by.rs`, `window.rs`, `cte.rs`, `set_ops.rs`, `execution.rs`, `tests.rs`).
  - Decomposed `crates/rurqe-core/src/expr.rs` (~1900 LOC) into modular facade and submodules (`val.rs`, `ast.rs`, `builder.rs`, `ops.rs`, `functions.rs`, `render.rs`, `tests/`).
  - Decomposed `crates/rurqe-migrate/src/catalog.rs` (~901 LOC) into modular facade and submodules (`types.rs`, `lock.rs`, `schema.rs`, `tests.rs`).
  - Decomposed `crates/rurqe-postgres/src/lib.rs` (~1016 LOC) into modular facade, `dialect.rs`, and `backend/{types, async_conn, sync_conn, pools}.rs`.
  - Decomposed `crates/rurqe-cli/src/introspect.rs` (873 LOC) into modular facade and `introspect/{types, sqlite, postgres, tests}.rs`.
  - Decomposed `crates/rurqe-cli/src/cli.rs` (821 LOC) into modular facade and `cli/{args, connection, commands, tests}.rs`.
  - Zero `mod.rs` files; clean Rust 2018+ module layout across the entire codebase.
- [x] **Strict Panic-Free Production Invariant:**
  - Audited and eliminated unhandled runtime panics in production crates.
  - Replaced runtime asserts in DML mutations (`insert.rs`, `update.rs`) with compile-time Typestate guards (`InsertSource<T>`, `AssignmentsPresent`).
  - Replaced unhandled panics/expects in `rurqe-migrate` parsing paths (`migration.rs`, `catalog/schema.rs`) with contextual `Result` handling.
- [x] **Observability Transition:**
  - Tracing facade integration deferred to Phase 14 to unify logging across drivers, pool managers, and the expanded rich types layer.

---

## Phase 14: Rich Types, Custom Domain Types & Observability (Ecosystem Features, Newtypes & Tracing) [Completed]

### Objective
Expand the RURQE type system beyond primitive scalars to support standard Rust ecosystem types via modular Cargo feature flags (`uuid`, `serde_json`, `chrono`, `time`) without dependency bloat; introduce the lightweight `tracing` observability facade across database drivers, pools, transactions, and migration runners; and provide first-class, zero-boilerplate ergonomics for custom domain types (binary and text newtypes such as `bitcoin::Txid` or `bitcoin::Address`, as well as domain enums) with strict decode validation, zero runtime panics, and CLI code generation support.

### Key Deliverables
- [x] **Observability & Logging Facade (`tracing`):**
  - Integrate the `tracing` facade (`info!`, `warn!`, `error!`, `trace!`) in `rurqe-postgres`, `rurqe-sqlite`, `rurqe-migrate`, and the `rurqe` facade. The default dependency graph of the zero-overhead `rurqe-core` carries no observability dependency: the lifecycle events of `TransactionGuard` are emitted into the `tracing` facade behind the optional `tracing` Cargo feature (default off), which the `rurqe` facade enables for its consumers, so a direct consumer of `rurqe-core` builds without `tracing` in its graph while the events only the guard observes — savepoint nesting and connection poisoning — remain available through the facade.
  - Instrument connection lifecycles, transaction boundaries (`BEGIN`, `COMMIT`, `ROLLBACK`), savepoints, and migration executions (`V...`, `R...`).
- [x] **Modular Cargo Feature Flags:**
  - `feature = "uuid"`: support for `uuid::Uuid` across AST expressions, DML mutations, row decoding, and zero-allocation binary protocol bindings in Postgres and SQLite.
  - `feature = "serde_json"`: support for `serde_json::Value` and a transparent wrapper `Json<T: Serialize + DeserializeOwned>` for JSON and JSONB columns.
  - `feature = "chrono"`: support for `chrono::NaiveDateTime`, `chrono::DateTime<Utc>`, and `chrono::NaiveDate`.
  - `feature = "time"`: alternative temporal types via `time::OffsetDateTime` and `time::Date`.
- [x] **Custom Domain Types & Newtypes Integration Contract:**
  - Standardized conversion contracts (`ToSqlVal` and `FromSqlValue`) supporting custom domain types with fail-fast validation and typed error propagation (`ExecutionError::RowDecode`) without runtime panics.
  - Transparent conversion for binary types (`AsRef<[u8]>`, `TryFrom<&[u8]>`) and string-backed types (`Display`, `FromStr`).
- [x] **Ergonomic Bridge Macros for Custom Types:**
  - Declarative helper macros `impl_sql_binary_newtype!(Type, expected_len)` and `impl_sql_text_newtype!(Type)` to instantly connect third-party types (e.g. `bitcoin::Txid`, `bitcoin::Address`, hash digests) with zero boilerplate.
  - Derive macro support (`#[derive(SqlType)]`) for domain newtypes and string/integer-backed enums.
- [x] **CLI Codegen Custom Type Overrides (`rurqe.toml`):**
  - Configuration support in CLI codegen (`[codegen.type_overrides]`) to map raw column types (`BYTEA`, `BLOB`, `VARCHAR`) directly to custom Rust types in the generated `schema.rs`.
- [x] **Driver Protocol Integration & Verification:**
  - High-performance, zero-allocation native driver encoding/decoding where available (e.g. `postgres-types/with-uuid-1`), falling back to typed binary/text representations.
  - End-to-end integration tests covering queries, DML, `select!` projections, and error reporting on invalid payload decoding.

### Example Usage
```rust
use bitcoin::{Address, Txid};
use rurqe::postgres::prelude::*;

// 1. One-line bridge macro for third-party domain types
rurqe::impl_sql_binary_newtype!(Txid, 32);
rurqe::impl_sql_text_newtype!(Address);

// 2. Strict type safety in schema and queries
let tx = select!(
    tx_id: Transactions::TxId,
    recipient: Transactions::Recipient,
    amount: Transactions::Amount,
)
.from(Transactions)
.where_(Transactions::TxId.eq(target_txid))
.fetch_optional(&mut conn)?;
```

---

## Phase 15: Stage-Based Schema Evolution (Unified Stage + Migration Tail) [Completed]

### Objective
Unify declarative schema design with sequential migration safety in a **Grand Unified Model**: maintain a single current declarative DDL state (`schema/`) as the Single Source of Truth for developers and AI, alongside an active tail of sequential migrations (`migrations/`) for upgrading existing databases with data transformations. Enable instant cold-start database initialization directly from `schema/` for tests and CI, introduce cryptographic checkpoint markers for zero-pain migration pruning without squashing, mathematically verify consistency via **The Golden Equation** ($\text{State}(\text{schema/}) \equiv \text{State}(\text{Checkpoint} + \text{migrations/})$), and preserve 100% compatibility for classic Flyway-style workflows.

### Key Deliverables
- [x] **Dual Lifecycle Modes (Traditional vs. Stage+Tail):**
  - Traditional mode: sequential execution of versioned (`V...`) and repeatable (`R...`) scripts directly from `migrations/`.
  - Stage+Tail mode: cold-start bootstrap from `schema/` for clean databases, or sequential application of `migrations/` for existing databases.
- [x] **Declarative Target Schema (`schema/`) & Instant Cold-Start:**
  - Single Source of Truth directory containing complete declarative DDL (e.g. `schema/users.sql`, `schema/billing.sql`).
  - Cold-start bootstrap: instant database initialization from `schema/` in a single transaction, recording a synthesized applied marker in `__rurqe_schema_history` matching the active stage checksum.
- [x] **Checkpoint Markers & Zero-Pain History Pruning:**
  - Checkpoint metadata (`checkpoint.toml` or migration header) storing `pruned_up_to_version` and base `stage_hash`.
  - Freedom to delete historical migration files prior to the checkpoint (`rm -f migrations/V001..V050*.sql`) without rewriting baselines or invalidating checksums.
  - Fail-safe guard: migration runner aborts with an actionable error if an existing target database version is older than the pruned checkpoint boundary.
  - Boundary snapshots recorded by the toolchain: `stage commit` copies the declarative stage verbatim into a `checkpoint-stage/` directory next to `checkpoint.toml`, `stage verify` loads it as the base stage when `--base-stage-dir` is omitted, and a checkpoint whose non-empty boundary has no base stage is rejected with `StageError::BaseStageMissing` instead of replaying the tail over an empty scratch database.
- [x] **Golden Equation Verification Engine (`cargo db-verify`):**
  - Verification workflow asserting $\text{State}(\text{schema/}) \equiv \text{State}(\text{Checkpoint} + \text{migrations/})$:
    1. Spin up ephemeral DB 1 $\to$ apply `schema/` DDL $\to$ snapshot catalog $K_{stage}$.
    2. Spin up ephemeral DB 2 $\to$ apply Checkpoint base + active `migrations/` $\to$ snapshot catalog $K_{tail}$.
    3. Compare $K_{stage}$ and $K_{tail}$ bit-for-bit using `SchemaDiff`. Fail with exact divergence report on mismatch.
- [x] **Stage & Pruning Developer Tooling in `rurqe-cli`:**
  - `cargo db stage init`: bootstrap declarative `schema/` and `schema.lock` from an existing database or migration set.
  - `cargo db stage commit -m "..."`: introspect differences, generate next migration draft in `migrations/`, and synchronize `schema/`.
  - `cargo db prune --before <version>`: create or advance the checkpoint marker and prune obsolete migration files safely.

### Conceptual Layout
```text
project/
├── schema/                       # Complete declarative target DDL (SSOT for AI & Devs)
│   ├── users.sql
│   └── billing.sql
├── migrations/                   # Active migration tail (Delta for live DBs)
│   ├── checkpoint.toml           # Pruned up to V050, base_hash = "sha256:..."
│   ├── checkpoint-stage/         # Verbatim DDL snapshot of the checkpoint boundary
│   ├── V051__add_credits.sql     # Active DDL + DML migration
│   └── V052__index_billing.sql   # Active DDL migration
├── schema.lock                   # Deterministic catalog snapshot (SCHEMA_HASH)
└── rurqe.toml                    # Configuration (mode = "stage_tail" | "migrations")
```

---

## Phase 16: Compile-Fail Hardening & Negative Test Suite [Completed]

### Objective
Make the type system the only gatekeeper of **Absolute SQL Correctness**: every query that is syntactically or semantically invalid for the target dialect and version must be rejected by `rustc`, and every rejection must name the violated SQL rule through `#[diagnostic::on_unimplemented]` instead of exposing trait cascades. The phase consolidates the guarantees delivered by the preceding slices into one auditable catalogue of `compile_fail` doc-tests, so weakening an invariant breaks `cargo test --doc` instead of reaching a database.

### Key Deliverables
- [x] **Relational scope isolation (`#c-relational-scope`):**
  - Every expression reachable from `select`, `where_`, `inner_join` / `left_join` `ON`, `having`, `group_by`, `order_by`, `distinct_on`, window specifications, `SET` assignments, and `RETURNING` is checked against the relations the statement actually introduces (`FROM` plus every join).
  - Referencing a column of an unjoined table, comparing columns of unrelated relations, and projecting or returning a foreign table's column are rejected at the call site with the `TableInScope` / `ExprInScope` diagnostics.
- [x] **Outer-join nullability soundness (`#c-nullability-soundness`):**
  - Columns reachable through `LEFT JOIN` and `LEFT JOIN LATERAL` are strictly typed as `Option<T>`, enforced by `TableNullabilityInScope`, `ApplyNullability`, and `MakeNullable` for every scalar, domain newtype, and optional column.
  - Reading such a column as a plain `T` fails to compile, so a missing row can never be decoded into a non-nullable field.
- [x] **Typestate guards (`#c-typestate-soundness`):**
  - `SELECT` without a projection (`ProjectionPresent`), `UPDATE` / `DELETE` without `WHERE` or an explicit `.allow_all()` (`ExecutableMutation`), `UPDATE` without `SET` (`AssignmentsPresent`), `INSERT` without rows or a source `SELECT` (`InsertSource`), an empty row source (`EmptyRows`), and a `CASE` finalised without a `WHEN` branch (`NoBranches`) are all unreachable by construction.
- [x] **Capability gating (`#c-capability-gating`):**
  - `RETURNING`, `ON CONFLICT`, `UPDATE ... FROM`, `DELETE ... USING`, `DISTINCT ON`, and `LATERAL` exist only for dialects and versions implementing the corresponding capability marker, so an unsupported clause cannot even be named on an incompatible target.
- [x] **Aggregation soundness (`#c-group-by-soundness`):**
  - After `.group_by(...)` moves the query into `GroupedQuery`, only grouped columns, aggregates, and constant or window expressions are projectable; the `Expr::column` escape hatch does not bypass `GroupedSelectTarget`.
- [x] **Unified negative suite (`#c-negative-test-suite`):**
  - 36 `compile_fail` doc-tests catalogue every rejection class below, each one paired with a positive example of the accepted form.
  - The workspace compiles cleanly under `cargo clippy --workspace --all-targets -- -D warnings`, and `cargo test --doc` runs the whole catalogue as part of the standard test surface.

### Compile-Fail Catalogue

| Invariant | Guard traits | Cases | Rejected at compile time |
|---|---|---|---|
| `#c-relational-scope` | `TableInScope`, `ExprInScope`, `ScopePositions`, `ColumnInScope`, `SelectTargetInScope`, `GroupByInScope`, `TableSelection`, `ColumnBelongsTo` | 14 | projecting, filtering, grouping, ordering, windowing, joining, assigning, or returning a column of a relation absent from `FROM` / `JOIN`; comparing columns of two unrelated relations |
| `#c-nullability-soundness` | `TableNullabilityInScope`, `ApplyNullability`, `MakeNullable` | 1 | reading a `LEFT JOIN` / `LEFT JOIN LATERAL` column as `T` instead of `Option<T>` |
| `#c-typestate-soundness` | `ProjectionPresent`, `ExecutableMutation`, `AssignmentsPresent`, `InsertSource`, `EmptyRows` | 13 | `SELECT` without projection, unguarded `UPDATE` / `DELETE`, `INSERT` without a row source, `INSERT` from an empty row source, `UPDATE` without `SET`, `CASE` without `WHEN` |
| `#c-capability-gating` | `SupportsReturning`, `SupportsOnConflict`, `SupportsUpdateFrom`, `SupportsDeleteUsing`, `SupportsLateralJoin` | 5 | `RETURNING` and `ON CONFLICT` below their SQLite version thresholds, `UPDATE ... FROM` below 3.35, `DELETE ... USING` and `LATERAL` on SQLite at all |
| `#c-group-by-soundness` | `GroupedSelectTarget`, `ColumnInGroupBy` | 3 | projecting a column that is neither grouped nor an aggregate, including attempts routed through `Expr::column` |

The catalogue is distributed across the API surface it protects:

| Doc-test host | Cases | Focus |
|---|---|---|
| `crates/rurqe/src/lib.rs` | 17 | facade entry points: scope, typestate, mutation scope |
| `crates/rurqe-core/src/schema.rs` | 2 | `TableInScope` and `ExprInScope` contracts |
| `crates/rurqe-core/src/query/selection.rs` | 1 | outer-join nullability lifting |
| `crates/rurqe-core/src/query/group_by.rs` | 3 | grouped projections and window scope |
| `crates/rurqe-core/src/query/select.rs` | 1 | projection typestate |
| `crates/rurqe-core/src/dml.rs` | 2 | `ExecutableMutation`, `AssignmentsPresent` |
| `crates/rurqe-core/src/dml/insert.rs` | 2 | `InsertSource` |
| `crates/rurqe-core/src/dml/update.rs` | 1 | unfiltered `UPDATE` |
| `crates/rurqe-core/src/dml/delete.rs` | 1 | unfiltered `DELETE` |
| `crates/rurqe-core/src/expr/ast.rs` | 1 | `CASE` builder typestate |
| `crates/rurqe-sqlite/src/lib.rs` | 5 | version-gated SQLite capabilities |

### Example Usage
```rust
use rurqe::prelude::*;

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

// Rejected at compile time: `OrderTotal` belongs to `Order`, which the query
// never introduces, so the error names the missing relation instead of leaking
// a trait resolution cascade.
let _ = select_from::<Postgres, User>()
    .select((UserId, OrderTotal));
```

```text
error[E0277]: column `OrderTotal` cannot be projected from this query's scope
  = note: introduce the column's table with `from` or a join before projecting it
```

---

## Phase 17: Relational Depth, Row Expressions & Locking (jOOQ Parity & Transactional Soundness) [Completed]

### Objective
Deliver deep relational expressiveness and strict transactional concurrency without synthetic runtime emulation: row value expressions (column tuples in comparisons, predicates, and keyset pagination), scalar subqueries seamlessly lifting into typed `Expr<T>`, pessimistic row-level locking (`FOR UPDATE`, `FOR SHARE`, `SKIP LOCKED`, `NOWAIT`) gated by dialect capability traits, ANSI SQL:2003 aggregate `FILTER (WHERE ...)`, complete window frame specifications (`ROWS/RANGE BETWEEN`), and strictly typed references to the `EXCLUDED` pseudo-relation in `ON CONFLICT DO UPDATE`.

### Key Deliverables
- [x] **Row Value Expressions (SQL:1992 Tuple Comparisons & Keyset Pagination):**
  - Column tuples as first-class operands: `(col_a, col_b).eq((val_a, val_b))`, `(col_a, col_b).gt((val_a, val_b))`.
  - Tuple membership predicates: `(col_a, col_b).in_subquery(subquery)`.
  - High-level keyset/cursor pagination DSL: `.paginate_after((created_at, id), cursor)` translating directly into native tuple ordering comparisons (`>` / `<`) with deterministic index utilization in PostgreSQL and SQLite.
- [x] **Scalar Subqueries as First-Class Expressions (`IntoExpr<T>` & `Expr<T>`):**
  - Direct implementation of `IntoExpr<T>` and `SqlCompatible<T, SelectQuery<...>>` for single-column `SelectQuery`: subqueries pass directly into comparison operators without any conversion method (`ProductPrice.gt(select_from(Products).select(avg(ProductPrice)))`).
  - Standard `From<SelectQuery<...>> for Expr<T>` implementation enabling `.into()` whenever the target expression type is unambiguously inferred.
  - Explicit `Expr::subquery(query)` constructor for unconstrained syntactic positions (`CASE WHEN`, tuple comparisons).
  - Static invariant: subqueries projecting multi-column tuples do not implement `IntoExpr<T>` and are rejected at compile time.
- [x] **Pessimistic Row-Level Locking (`SupportsRowLocking`):**
  - Gated by the `SupportsRowLocking` capability trait (implemented for `Postgres<V>`, absent for `Sqlite`).
  - Query locking methods: `.for_update()`, `.for_share()`, `.for_no_key_update()`, `.for_key_share()`.
  - Lock acquisition modifiers: `.nowait()`, `.skip_locked()`.
  - Zero emulation: SQLite usage rejected at compile time via `#[diagnostic::on_unimplemented]`.
- [x] **Aggregate `FILTER (WHERE ...)` (SQL:2003) & Aggregate `DISTINCT`:**
  - Condition filtering on aggregates: `count_all().filter_where(condition)`, `sum(amount).filter_where(condition)`.
  - Gated via `SupportsAggregateFilter` (PostgreSQL 9.4+, SQLite 3.30+). Native rendering without `CASE WHEN` emulation.
  - Typed distinct aggregates: `count_distinct(col)`, `sum_distinct(col)`.
- [x] **Window Frame Specifications (`ROWS / RANGE BETWEEN`):**
  - Complete window framing DSL on `WindowSpec`: `.rows_between(start, end)`, `.range_between(start, end)`.
  - Frame bounds: `WindowFrame::UnboundedPreceding`, `WindowFrame::Preceding(n)`, `WindowFrame::CurrentRow`, `WindowFrame::Following(n)`, `WindowFrame::UnboundedFollowing`.
- [x] **Strictly Typed `EXCLUDED` in `ON CONFLICT DO UPDATE`:**
  - Typed reference to rejected row values via `excluded(Col)` in upsert assignments:
    `on_conflict(UserId).do_update((UserName.set(excluded(UserName)), Attempts.set(Attempts.add(1))))`.
  - Renders `\"users\".\"name\" = EXCLUDED.\"name\"` (native in PostgreSQL and SQLite 3.24+).

### Example Usage
```rust
use rurqe::postgres::prelude::*;

// 1. Pessimistic row locking with SKIP LOCKED for transactional job queues
let next_job = select_from(Jobs)
    .select(cols!(JobId, JobPayload))
    .where_(JobStatus.eq("pending"))
    .order_by_asc(JobCreatedAt)
    .limit(1)
    .for_update()
    .skip_locked()
    .fetch_optional(&mut conn)?;

// 2. Row value expressions for cursor-based keyset pagination
let next_page = select_from(Events)
    .select(cols!(EventId, EventCreatedAt, EventData))
    .where_((EventCreatedAt, EventId).lt((cursor_time, cursor_id)))
    .order_by_desc((EventCreatedAt, EventId))
    .limit(50)
    .fetch_all(&mut conn)?;

// 3. Direct scalar subquery in comparison operator without .into_scalar_expr()
let above_average = select_from(Products)
    .select(cols!(ProductId, ProductName, ProductPrice))
    .where_(ProductPrice.gt(
        select_from(Products).select(avg(ProductPrice))
    ))
    .fetch_all(&mut conn)?;

// 4. Aggregate FILTER (WHERE ...)
let stats = select_from(Orders)
    .select((
        count_all().filter_where(OrderStatus.eq("completed")),
        sum(OrderTotal).filter_where(OrderStatus.eq("refunded")),
    ))
    .fetch_one(&mut conn)?;
```

---

## Phase 18: Kysely DX, Dynamic Builders & Nested DTOs (MULTISET via Native JSON) [Completed]

### Objective
Achieve Kysely-grade developer ergonomics without runtime type erasure or untyped JSON blobs: typed nested collection/record synthesis (`nest_many` / `nest_one`) directly from SQL using native database JSON aggregates (eliminating N+1 queries and Cartesian join explosions), fluent conditional query construction (`.when`, `.if_some`, `.where_opt`) preserving typestate, typed derived tables (`.as_table(...)` / `from_subquery(...)`), database-first schema enums (PostgreSQL `pg_enum` and SQLite `CHECK`), and first-class PostgreSQL array types (`T[]`).

### Key Deliverables
- [x] **Typed Nested Results (MULTISET / Kysely `jsonArrayFrom` & `jsonObjectFrom`):**
  - First-class `nest_many(subquery)` and `nest_one(subquery)` projection targets in `select!` and `dto!`.
  - Native single-round-trip SQL generation without Cartesian join multiplication or client-side group-by stitching:
    - PostgreSQL: `COALESCE(jsonb_agg(to_jsonb(sub)), '[]'::jsonb)` or `json_build_object(...)`.
    - SQLite: `COALESCE(json_group_array(json_object(...)), '[]')`.
  - Zero-boilerplate row mapping: synthesizes fields typed as `Vec<ChildDto>` or `Option<ChildDto>` with direct single-pass deserialization.
- [x] **Fluent Conditional Query Composition (Typestate-Preserving Dynamic DX):**
  - Dynamic chaining without breaking method chains or losing compile-time guarantees:
    - `.when(condition, |q| q.where_(...))`
    - `.if_some(opt_value, |q, val| q.where_(...))`
    - `.where_opt(Option<Expr<bool>>)`
  - Strictly preserves query typestate (`Filtered` remains `Filtered`; transitioning from `Unfiltered` to `Filtered` is seamless).
- [x] **Derived Tables (`from_subquery` / `.as_table(...)`):**
  - Turning a projected `SelectQuery` into a relation: `let top_users = select_from(Users)...as_table("top_users");`.
  - Participates in `TableScope`, `ExprScope`, and joins with typed column access.
- [x] **DB-First Enums (`pg_enum` & SQLite `CHECK`):**
  - CLI introspection of PostgreSQL `CREATE TYPE ... AS ENUM` and SQLite `CHECK (col IN (...))` constraints into `schema.lock` (`[[enum]]`).
  - Codegen emits typed `#[derive(SqlType, Clone, Copy, PartialEq, Eq, Debug)] pub enum ...` with compile-time type checking in expressions: `UserStatus.eq(Status::Active)`.
- [x] **Native PostgreSQL Array Types (`T[]`):**
  - Introspection and mapping of array columns (`_text` $\to$ `Vec<String>`, `_int4` $\to$ `Vec<i32>`).
  - Dialect extension trait `PgArrayExpressions`: array containment (`@>`), overlap (`&&`), and `col.any(subquery)` / `col.all(subquery)`.
  - Purely in `rurqe-postgres`, zero pseudo-emulation on SQLite.

### Example Usage
```rust
use rurqe::postgres::prelude::*;

// 1. Typed nested DTOs (MULTISET): One query, zero Cartesian joins, typed Vec<OrderSummary>
let users_with_orders = select_from(Users)
    .select(dto!(UserWithOrders {
        user_id: UserId,
        user_name: UserName,
        orders: nest_many(
            select_from(Orders)
                .select(dto!(OrderSummary {
                    order_id: OrderId,
                    total: OrderTotal,
                }))
                .where_(OrderUserId.eq(UserId))
        ),
    }))
    .fetch_all(&mut conn)?;

// 2. Fluent dynamic query construction without typestate breakage
let filtered = select_from(Users)
    .select(cols!(UserId, UserName))
    .when(only_active, |q| q.where_(IsActive.eq(true)))
    .if_some(search_term, |q, term| q.where_(UserName.ilike(format!("%{term}%"))))
    .fetch_all(&mut conn)?;
```

---

## Phase 19: Expression Polish, Practical DML & Advanced Ordering [Completed]

### Objective
Eliminate the remaining need for untyped raw SQL escape hatches (`sql!`) in common OLTP and fintech workloads: enable intuitive Rust arithmetic operators (`+`, `-`, `*`, `/`, `%`) directly on typed expressions and columns via `std::ops`, complete upsert expressiveness with partial conflict targets and conditional updates (`ON CONFLICT (...) WHERE ... DO UPDATE ... WHERE ...`), provide explicit deterministic null ordering (`NULLS FIRST` / `NULLS LAST`), integrate `rust_decimal::Decimal` under the `decimal` feature flag, implement core ANSI scalar functions (`cast`, `nullif`, `greatest`, `least`, `is_distinct_from`), add PostgreSQL JSONB value extraction operators (`->`, `->>`), and complete relational join coverage with unconditional `CROSS JOIN` and strictly typed `FULL OUTER JOIN` (with bilateral compile-time `Option<T>` nullability lifting).

### Key Deliverables
- [x] **Arithmetic Operators via `std::ops` on Expressions & Columns:**
  - Implementation of `Add`, `Sub`, `Mul`, `Div`, `Rem` for `Expr<T, Scope>` and `Column` markers.
  - Scope propagation across operands via `MergeScope`. Compile-time numeric type matching (`i32`, `i64`, `f64`, `Decimal`).
- [x] **Full Upsert Expressiveness (`ON CONFLICT WHERE` & `DO UPDATE WHERE`):**
  - Partial unique index target predicates: `on_conflict_where(target, target_predicate)`.
  - Conditional update predicates: `do_update_where(assignments, update_predicate)`.
  - Native rendering for PostgreSQL and SQLite 3.24+ without emulation.
- [x] **Explicit Null Ordering (`NULLS FIRST / NULLS LAST`):**
  - Fluent modifiers on `OrderDirection` / `SelectQuery` / `WindowSpec`: `.order_by_asc_nulls_first(...)`, `.order_by_desc_nulls_last(...)`, etc.
  - Deterministic index and keyset pagination support across varying database defaults.
- [x] **Ecosystem Decimal Integration (`rust_decimal` under feature `decimal`):**
  - Implement `ToSqlVal`, `FromSqlValue`, `SqlArrayElement`, and `MakeNullable` for `rust_decimal::Decimal`.
  - Connect with the existing base-10000 binary PostgreSQL `NUMERIC` codec in `rurqe-postgres`.
  - Codegen configuration mapping `TypeKind::Numeric` to `rust_decimal::Decimal` when the feature is enabled.
- [x] **Standard SQL Scalar Functions & Null Predicates:**
  - Type-safe helpers in `expr::functions`: `cast::<Target>(expr)`, `nullif(a, b)`, `greatest(...)`, `least(...)`, `is_distinct_from(a, b)`, `is_not_distinct_from(a, b)`.
- [x] **PostgreSQL JSONB Field Extraction Operators:**
  - Extension methods in `PgExpressions`: `->` (extract JSON object/element), `->>` (extract text scalar), `#>` / `#>>` (extract path).
- [x] **Relational Join Completeness (`CROSS JOIN` & `FULL OUTER JOIN`):**
  - Unconditional `.cross_join(table)`.
  - `.full_outer_join(table, condition)`: static bilateral nullability lifting (both left and right relations lift to `Option<T>` in result projections).

---

## Phase 20: Absolute SQL Soundness: Relational Scoping, Grouping & Ordering Typestates [Completed]

### Objective
Fulfill the core vision of **Absolute SQL Correctness** (`Vision.md § 1.5.1`) by closing the remaining classes of runtime-failing SQL across both tiers of the guarantee: keep strict `IN` subquery arity and type congruence, grouping provenance through scalar expressions, and compound-statement ordering inside the stable-Rust type system, while the rules that need a negative assertion (a relation is *absent* from the scope) or the alignment of one clause with another one written later in a dynamic fluent chain (`DISTINCT ON` prefix, compound `ORDER BY` targets) are validated deterministically by the fallible `.build()` before any network I/O. No invalid statement reaches the engine, and nothing relies on nightly-only type-system features.

### Key Deliverables
- [x] **Disjoint Relation Scope & Unaliased Duplicate Join Prevention (tier 2):**
  - Two relations sharing one effective name in `FROM` / `JOIN` are rejected deterministically during `.build()` with `BuildError::DuplicateRelation { name }`, which names the colliding relation and points at `table_alias!` / `.as_alias(...)` as the remedy.
  - The effective name is the alias a relation is referenced under, or its table name when it carries none, so `FROM "users" JOIN "users"` collides exactly like two uses of one alias; derived tables and lateral subqueries participate under the marker name they are exposed as.
  - On stable Rust a compile-time disjointness guard would require a negative assertion (`#![feature(negative_impls)]`), which the stable-Rust invariant forbids, so the rule lives in tier 2 of `Vision.md § 1.5.1` — deterministic, before any SQL reaches the engine.
- [x] **Strict Subquery Arity & Type Soundness for `IN` Predicates (tier 1, delivered earlier):**
  - `InSubquery<Row>` ties the expected row type to the projection of the nested statement, so scalar `.in_subquery(sub)` and `.not_in_subquery(sub)` accept only a `SelectQuery` projecting exactly `(T,)` for an `Expr<T>` / `Column<RustType = T>`.
  - Row-value tuple predicates `(c1, c2).in_subquery(sub)` accept only subqueries whose projection tuple matches the exact arity and types `(T1, T2)`; a multi-column or arity-mismatched projection is rejected by `rustc`.
- [x] **Sound Grouped Expression Propagation in Scalar Functions (tier 1):**
  - `FunctionOperand` carries the scope, the aggregation marker, and the dialect requirement of every argument, and `CombineAggr` folds them, so `coalesce`, `nullif`, `greatest`, and `least` stay `Aggregate` as soon as one argument aggregates — `coalesce(sum(orders.total), 0.0)` projects from a `GroupedQuery`.
  - `CaseBuilder` folds the markers of its conditions, its results, and its `ELSE` fallback the same way, so a `CASE` written over aggregates stays projectable while a branch reading a bare column makes the whole expression per-row.
  - An expression that reads an ungrouped column therefore does not implement `GroupedSelectTarget`, and the grouping bypass is rejected by the compiler instead of by the engine.
- [x] **Ordering Soundness for `DISTINCT` & `DISTINCT ON`:**
  - The `DISTINCT` rule — every `ORDER BY` expression has to appear in the projection list — is enforced during `.build()` as well: the rule compares the assembled expressions rather than their types, so an ordering criterion naming no projected expression fails with `BuildError::DistinctOrderNotProjected { position }` before any SQL reaches the engine.
  - PostgreSQL `DISTINCT ON (cols)` prefix alignment is enforced during `.build()`: the leading `ORDER BY` expressions must match the `distinct_on` expressions left-to-right, otherwise the statement fails with `BuildError::DistinctOnOrderMismatch { position }` before any network I/O.
  - Deduplicating a statement moves it out of the locking typestate, so `FOR UPDATE` / `FOR SHARE` stop applying to it.
- [x] **Compound Query (`UNION`, `INTERSECT`, `EXCEPT`) `ORDER BY` Typestate:**
  - Combining statements moves the query into the `Compound` scope with the locking typestate unavailable, so the table-qualified `.order_by*` family — which would name a relation the combined statement no longer declares — does not apply.
  - Ordering the combined rows goes through `order_by_output(_asc/_desc/_nulls_first/_nulls_last…)` for output column names and `order_by_index(_asc/_desc)` for 1-based positions.
  - The first compound criterion moves the query into `TailClosed`, where `SetOperand` has no implementation, so no operand can follow the ordering of the combined rows.
  - `.build()` checks the assembled clause: `BuildError::CompoundOrderIndex` rejects a position outside the projection list and `BuildError::CompoundOrderColumn` rejects an output name the projection list does not produce.

### Example Usage
```rust
use rurqe::postgres::prelude::*;

// 1. Rejected while building (tier 2): two relations share one effective name
// let err = select_from(users).inner_join(users, users.id.eq(users.id)).select(cols!(users.id)).build().unwrap_err();
// assert!(matches!(err, BuildError::DuplicateRelation { name: "users" }));
// Fix: alias the second occurrence — `let u2 = users.as_alias("u2");`

// 2. Rejected at compile time (tier 1): arity mismatch in an IN subquery
// let _ = users.id.in_subquery(select_from(orders).select((orders.id, orders.user_id))); // ERROR: expected (i64,), got (i64, i64)

// 3. Rejected at compile time (tier 1): ungrouped column hidden inside coalesce
// let _ = select_from(users)
//     .group_by(users.id)
//     .select((users.id, coalesce(users.name, "anon"))); // ERROR: users.name is neither grouped nor aggregated

// 4. Guaranteed valid SQL: DISTINCT with matching ORDER BY
let unique_names = select_from(users)
    .select(users.name)
    .distinct()
    .order_by_asc(users.name)
    .fetch_all(&mut conn)?;

// 5. Guaranteed valid SQL: a compound statement ordered by its own output columns
let combined = select_from(users)
    .select(cols!(users.name))
    .union_all(select_from(archived_users).select(cols!(archived_users.name)))
    .order_by_output_asc(users.name)
    .fetch_all(&mut conn)?;
```

---

## Backlog: Future Architectural Horizons [Post-0.1.0]

The following capabilities represent valuable, high-utility SQL features that require specialized multi-step type designs, dedicated typestates, or engine-specific handling, and are scheduled for evolution cycles after the 0.1.0 core stabilization:

1. **Recursive Common Table Expressions (`WITH RECURSIVE`):**
   - Type-safe recursive step validation: self-referential CTE inside `UNION ALL` ensuring strict compile-time column type congruence between anchor and recursive queries.
2. **Data-Modifying Common Table Expressions:**
   - CTEs containing `INSERT ... RETURNING`, `UPDATE ... RETURNING`, or `DELETE ... RETURNING` driving the primary query. Capability-gated to PostgreSQL.
3. **Table Value Constructors (`VALUES` as a Relation):**
   - Using `FROM (VALUES (...), (...)) AS t(c1, c2)` as an ad-hoc query relation or join source for batch updates.
4. **Native `MERGE` Statement (SQL:2003 / PostgreSQL 15+):**
   - Strictly native `MERGE INTO ... USING ... WHEN MATCHED ... WHEN NOT MATCHED` implementation without synthetic emulation.
5. **PostgreSQL Range Types (`RANGE`):**
   - Native codecs and operators (`@>`, `<@`, `&&`, `-|-`) for `int4range`, `int8range`, `numrange`, `tsrange`, `tstzrange`, `daterange`.
6. **Temporal `TIME` (`chrono::NaiveTime`) and `INTERVAL` Types:**
   - Dedicated `SqlVal::NaiveTime` variant and structured PostgreSQL interval handling.
7. **MySQL Dialect & Driver Support (`rurqe-mysql`):**
   - Complete third dialect implementation: backtick identifier escaping, positional `?` placeholders, and driver execution adapters for `mysql` / `mysql_async`.
8. **TLS / SSL Connection Support in CLI (`rurqe-cli`):**
   - Modular TLS support (`postgres-rustls` or `postgres-native-tls`) behind an optional Cargo feature flag, enabling encrypted connections when `sslmode=require` or `sslmode=verify-full` is specified.
9. **Automated `schema.lock` Parity in Stage Verification:**
   - Extension to `cargo db stage verify` validating that the committed `schema.lock` snapshot matches the declarative stage catalog bit-for-bit without requiring a separate drift step.
10. **PostgreSQL 17 Dialect & Version Marker (`Pg17`):**
    - Dedicated version marker `Pg17` with updated default `Postgres<Pg17>`, gating new engine capabilities including SQL/JSON standard expressions (`JSON_TABLE`, etc.) and `MERGE ... RETURNING`.
---

## Architectural Exclusions & Anti-Patterns [Won't Do / YAGNI]

The following constructs contradict RURQE's foundational principles (**Native-or-Nothing**, **Compile-Time Soundness**, **KISS/YAGNI**) and will **not** be implemented:

1. **Synthetic Runtime Emulation of `MERGE`:**
   - Emulating `MERGE` on SQLite or older PostgreSQL versions via non-atomic `SELECT` + `INSERT/UPDATE` transactions is strictly forbidden. It creates subtle race conditions and violates the **Native-or-Nothing** principle.
2. **`ROLLUP`, `CUBE`, and `GROUPING SETS`:**
   - RURQE is designed for operational OLTP workloads, not analytical BI/OLAP. At the type level, `GROUPING SETS` dynamically injects `NULL` into previously non-nullable grouped columns for subtotal rows, causing a combinatorial explosion of nullability states and destroying `GROUP BY` compile-time soundness.
3. **`JOIN ... USING (column)` Syntax:**
   - Relational anti-pattern that merges two relation columns into a single unqualified identifier. This destroys unambiguous static relation qualification in `TableScope`. Explicit `ON a.col = b.col` is strictly preferred.
4. **Dynamic `when_join` Erasing Relation Types:**
   - Query relation scope is tracked via compile-time types (`SelectQuery<D, Joined<User, Order>, ...>`). Conditionally adding a table in a runtime `if` statement without boxing or type erasure is mathematically incompatible with static monomorphization. Optional relationships are properly modeled via `LEFT JOIN` or separate typed queries.
5. **Hundreds of Non-Standard String/Math Functions in Core:**
   - Bloating `rurqe-core` with niche functions (`LPAD`, `INITCAP`, `COT`, `DEGREES`) violates the 800-line module limit and core cleanliness. Engine-specific extensions belong in adapter crates (`rurqe-postgres`), while one-off functions are cleanly accessed via `sql!(\"custom_func({})\", col)`.
6. **`GROUPS` Window Frame Specification:**
   - Unsupported by SQLite and rarely utilized in production PostgreSQL. `ROWS` and `RANGE` cover ANSI SQL:2003 specifications and 100% of practical production windowing needs.
7. **External Lockfiles or Daemon Locks for SQLite Non-Transactional Migrations:**
   - Attempting to hold run-wide exclusive locks across autocommit statements (`VACUUM`, `PRAGMA`) in SQLite using external OS lockfiles (`.db.lock`), daemon coordination, or auxiliary table locks is strictly rejected as YAGNI. SQLite is an embedded single-writer database, not a high-concurrency client-server cluster; committing the `BEGIN IMMEDIATE` transaction to execute non-transactional maintenance and acquiring it again immediately afterwards is the correct, native SQLite design.

