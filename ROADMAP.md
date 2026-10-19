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
- [ ] **Conditional Expressions:**
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
