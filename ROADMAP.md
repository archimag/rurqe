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
| **Phase 12** [ ] | Two-Tier Architecture & High-Level SQL DSL | Value-based schema, zero-turbofish queries, automatic nullability, universal record synthesis | Two-Tier Architecture & Reference SQL DSL (Planned) |
| **Phase 13** [x] | Codebase Hardening & Conventions Refactoring | Large module decomposition (<800 LOC), panic-free typestate | Production Rigor & Standard Compliance (Completed) |
| **Phase 14** [x] | Rich Types, Custom Domain Types & Observability | Ecosystem types (UUID, JSON, Chrono/Time) via features, custom binary/text newtypes, enums, tracing | Type System Completeness & Observability (Completed) |
| **Phase 15** [ ] | Stage-Based Schema Evolution | AI-first declarative schema stages, verifiable transition paths, optional history retention | Stage-Based Schema Evolution & AI-First Lifecycle (Planned) |

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

## Phase 12: Two-Tier Architecture & High-Level SQL DSL [Planned]

### Objective
Establish a clean two-tier architecture separating the internal monomorphized relational IR from the primary public SQL DSL. Eliminate type-system machinery leaks (turbofish on table/column markers, manual `.nullable()` lifting, tuple projections, repeated dialect generics) in favor of value-based schema arguments (`.from(users)`), dialect-scoped modules (`rurqe::postgres::*`), connection-bound query factories, automatic nullability inference from relation provenance, universal named record synthesis in `select!`, and compile-time `GROUP BY` soundness.

### Key Deliverables
- [ ] **Zero-Turbofish Value-Based DSL:**
  - Query builder methods accept zero-sized table values as arguments: `.from(users)` / `.from(Users)`, `.inner_join(orders, ...)` / `.left_join(orders, ...)`.
  - Type-safe DML entrypoints taking table arguments: `insert_into(users)`, `update(users)`, `delete_from(users)` without turbofish syntax.
  - Zero runtime overhead: ZST arguments compile away completely while allowing `rustc` to infer table types immediately.
- [ ] **Dialect Ergonomics & Connection Integration:**
  - Specialized dialect modules (`rurqe::postgres::prelude::*`, `rurqe::sqlite::prelude::*`) pre-binding the dialect so application queries never require `<Postgres>` annotations.
  - Turbofish-free named selection start: `select!(...).from(table)` resolves dialect automatically from the active dialect prelude without explicit type parameters.
  - First-class dialect version submodules with dedicated preludes (`rurqe::postgres::pg14::prelude::*`, `rurqe::sqlite::sqlite3_24::prelude::*`) binding specific engine versions and automatically enforcing compile-time capability gates (`SupportsReturning`, `SupportsOnConflict`, etc.) with zero turbofish noise.
  - Connection and transaction query methods (`conn.select_from(users)`, `conn.update(users)`) inferring dialect directly from the database handle.
- [ ] **Value-Based Schema & Unified Expression Model:**
  - Codegen and `table!` generate schema objects/values (`users.id`, `orders.total`) that directly implement `Expression<T>`, eliminating `Expr::column::<Col>()` turbofish calls.
  - Fluent operator and method calls directly on columns (`users.age.gt(18)`, `users.name.like("%admin%")`).
  - Value-based table aliasing (`let u1 = users.as_alias("u1");`) without raw `AliasedCol<A, C>` wrappers.
- [ ] **Automatic Nullability Inference on Outer Joins:**
  - Outer joins mark relation scopes (`NullableRelation<T>`); columns projected from outer joins automatically lift their types to `Option<T>` without explicit `.nullable::<Col>()` calls.
- [ ] **Universal Named Record Projections:**
  - Macro `select!` synthesizes strongly typed ad-hoc structs for any projection items (columns, computed expressions, aggregates, aliased fields), making named field access (`row.id`, `row.total`) universal and phasing out manual tuple indexing.
- [ ] **Systematic SQL Soundness (GROUP BY Validation):**
  - Typestate transition to `GroupedQuery` upon `.group_by(...)`, statically restricting projected fields to grouped columns or aggregate expressions, preventing runtime SQL grouping errors at compile time.
- [ ] **Domain-Specific Diagnostics & Error Reporting:**
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
  - Integrate the `tracing` facade (`info!`, `warn!`, `error!`, `trace!`) in `rurqe-postgres`, `rurqe-sqlite`, `rurqe-migrate`, and the `rurqe` facade without adding dependencies to the zero-overhead `rurqe-core`.
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

## Phase 15: Stage-Based Schema Evolution (Unified Stage + Migration Tail) [Planned]

### Objective
Unify declarative schema design with sequential migration safety in a **Grand Unified Model**: maintain a single current declarative DDL state (`schema/`) as the Single Source of Truth for developers and AI, alongside an active tail of sequential migrations (`migrations/`) for upgrading existing databases with data transformations. Enable instant cold-start database initialization directly from `schema/` for tests and CI, introduce cryptographic checkpoint markers for zero-pain migration pruning without squashing, mathematically verify consistency via **The Golden Equation** ($\text{State}(\text{schema/}) \equiv \text{State}(\text{Checkpoint} + \text{migrations/})$), and preserve 100% compatibility for classic Flyway-style workflows.

### Key Deliverables
- [ ] **Dual Lifecycle Modes (Traditional vs. Stage+Tail):**
  - Traditional mode: sequential execution of versioned (`V...`) and repeatable (`R...`) scripts directly from `migrations/`.
  - Stage+Tail mode: cold-start bootstrap from `schema/` for clean databases, or sequential application of `migrations/` for existing databases.
- [ ] **Declarative Target Schema (`schema/`) & Instant Cold-Start:**
  - Single Source of Truth directory containing complete declarative DDL (e.g. `schema/users.sql`, `schema/billing.sql`).
  - Cold-start bootstrap: instant database initialization from `schema/` in a single transaction, recording a synthesized applied marker in `__rurqe_schema_history` matching the active stage checksum.
- [ ] **Checkpoint Markers & Zero-Pain History Pruning:**
  - Checkpoint metadata (`checkpoint.toml` or migration header) storing `pruned_up_to_version` and base `stage_hash`.
  - Freedom to delete historical migration files prior to the checkpoint (`rm -f migrations/V001..V050*.sql`) without rewriting baselines or invalidating checksums.
  - Fail-safe guard: migration runner aborts with an actionable error if an existing target database version is older than the pruned checkpoint boundary.
- [ ] **Golden Equation Verification Engine (`cargo db-verify`):**
  - Verification workflow asserting $\text{State}(\text{schema/}) \equiv \text{State}(\text{Checkpoint} + \text{migrations/})$:
    1. Spin up ephemeral DB 1 $\to$ apply `schema/` DDL $\to$ snapshot catalog $K_{stage}$.
    2. Spin up ephemeral DB 2 $\to$ apply Checkpoint base + active `migrations/` $\to$ snapshot catalog $K_{tail}$.
    3. Compare $K_{stage}$ and $K_{tail}$ bit-for-bit using `SchemaDiff`. Fail with exact divergence report on mismatch.
- [ ] **Stage & Pruning Developer Tooling in `rurqe-cli`:**
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
│   ├── V051__add_credits.sql     # Active DDL + DML migration
│   └── V052__index_billing.sql   # Active DDL migration
├── schema.lock                   # Deterministic catalog snapshot (SCHEMA_HASH)
└── rurqe.toml                    # Configuration (mode = "stage_tail" | "migrations")
```
