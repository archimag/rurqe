# Getting Started

This guide demonstrates end-to-end usage of RURQE: establishing dependencies, declaring schema markers, executing typed reads, building mutations, managing transactions, and handling errors.

Query building performs no I/O and opens no network connection, allowing statements to compile without a reachable database. Every example in this chapter is verified by the workspace test suite.

## 1. Dependencies and Cargo Features

Add `rurqe` to `Cargo.toml` with the desired dialect and execution features:

```toml
[dependencies]
rurqe = { version = "0.1", features = ["sqlite", "macros"] }
# for PostgreSQL: features = ["postgres", "tokio-postgres", "deadpool-postgres", "macros"]
```

| Feature | Functionality |
|---|---|
| `sqlite` | Synchronous SQLite driver (`SqliteConnection`), version markers `Sqlite3_0` through `Sqlite3_38` |
| `postgres` | Synchronous PostgreSQL driver (`PgConnection`) based on `postgres` |
| `tokio-postgres` | Asynchronous PostgreSQL connection adapter (`PgAsyncConnection`) |
| `deadpool-postgres` | Asynchronous PostgreSQL connection pool (`PgAsyncPool`) |
| `macros` | Procedural and declarative macros: `select!`, `cols!`, `dto!`, `sql!`, `raw_sql!`, `#[derive(SqlType)]` |
| `migrate` | Embedded migration engine (`EmbeddedMigrations`, `embed_migrations!`) |
| `uuid`, `chrono`, `serde_json`, `decimal` | Ecosystem data type mappings |

## 2. Schema Markers

Schema definitions are represented by zero-sized marker types implementing the `Table` and `Column` traits:

```rust,no_run
{{#rustdoc_include examples/quickstart.rs:schema}}
```

In standard workflows, these markers are generated automatically by `cargo rurqe codegen` from the live database catalog, which writes the module and the catalog snapshot `rurqe/schema.lock` beside it.

## 3. Typed Read Queries

A filtered projection executed against an asynchronous connection pool:

```rust,no_run
{{#rustdoc_include examples/quickstart.rs:query}}
```

The result vector contains synthesized record structs whose fields match the projected column markers (`UserId` maps to `user_id`). Assembling the query chain opens no connection; execution occurs exclusively at the terminal call (`.fetch_all(&pool).await?`).

A projection names its items, so the fetched rows are read by those names: the field a chain reads is the one its projection declared, never a position in the row.

```rust,no_run
{{#rustdoc_include examples/quickstart.rs:named_rows}}
```

```sql
SELECT "id" AS "user_id", "name" AS "contact_email", "email" AS "user_email"
FROM "users"
WHERE ("email" = $1)
ORDER BY "id" ASC
```

A row that has to cross a function boundary is declared once with `dto!`, which reads every field type from the schema instead of restating it:

```rust,no_run
{{#rustdoc_include examples/quickstart.rs:dto}}
```

The declaration turns the values of such a row into the struct, and the struct is read by the names the declaration gave its fields:

```rust,no_run
{{#rustdoc_include examples/quickstart.rs:dto_row}}
```

## 4. Dynamic Query Construction

Conditional query modifiers allow runtime branches to augment query clauses without breaking the builder's typestate or erasing static types:

```rust,no_run
{{#rustdoc_include examples/quickstart.rs:conditional}}
```

When a flag is false or an `Option` contains `None`, the query remains unaffected and retains its static type.

## 5. Mutations: Insert, Update, Delete, and Upsert

All mutation statements enforce compile-time type compatibility and protective typestates:

```rust,no_run
{{#rustdoc_include examples/crud.rs:insert}}
```

Column assignments verify value types at compile time. Clauses such as `.returning(...)` are capability-gated and exist only for dialects supporting returning semantics.

Mutations require a filter predicate before execution is permitted, preventing accidental full-table operations:

```rust,no_run
{{#rustdoc_include examples/crud.rs:update}}
```

```rust,no_run
{{#rustdoc_include examples/crud.rs:delete}}
```

Conflict resolution is expressed natively via `on_conflict`:

```rust,no_run
{{#rustdoc_include examples/crud.rs:upsert}}
```

## 6. Joins and Automatic Nullability

Columns brought into scope via outer joins automatically lift their Rust types to `Option<T>`:

```rust,no_run
{{#rustdoc_include examples/joins.rs:outer}}
```

Because the right side of a `LEFT JOIN` may be absent, `order_total` evaluates to `Option<f64>`. Reading an outer-joined column as an unwrapped scalar is rejected by the compiler.

## 7. Aggregations and Grouping

Aggregations and grouped queries maintain strict SQL projection semantics:

```rust,no_run
{{#rustdoc_include examples/aggregates.rs:group_by}}
```

A grouped query accepts only grouped columns and aggregate expressions. Referencing an ungrouped column outside an aggregate function fails at compile time.

## 8. Nested Collections

One-to-many relationships can be queried and transformed into typed child collections in a single round trip:

```rust,no_run
{{#rustdoc_include examples/nested.rs:nested}}
```

The database engine performs JSON aggregation internally, and the deserializer maps the payload directly into a `Vec` of the synthesized child record — one `order_id`/`total` pair per child row — without Cartesian multiplication on the wire.

## 9. Transactions

Transactions use an RAII guard to guarantee atomicity. A transaction is opened on a live connection with `.transaction().await?`, and it commits when the handle awaits `commit().await?`; dropping the handle without committing rolls the transaction back, so a returned error or a cancelled future leaves no partial work behind.

```rust,no_run
{{#rustdoc_include examples/transactions.rs:transaction}}
```

## 10. Multi-Dialect Support and Version Gating

Importing a dialect prelude binds identifier quoting, parameter placeholders, and capability traits without generic annotations:

```rust,no_run
{{#rustdoc_include examples/dialects.rs:postgres}}
```

The equivalent SQLite query uses the SQLite prelude and generates positional `?` placeholders:

```rust,no_run
{{#rustdoc_include examples/dialects.rs:sqlite}}
```

Version-pinned preludes enforce capability boundaries. Clauses unsupported by the selected engine release do not exist in the builder API:

```rust,no_run
{{#rustdoc_include examples/dialects.rs:version}}
```

## 11. Pre-Flight Validation and Typestates

While the compiler enforces positive relational invariants, `.build()` deterministically validates semantic rules requiring negative assertions or multi-clause alignment before any network I/O:

```rust,no_run
{{#rustdoc_include examples/quickstart.rs:build_refusal}}
```

## 12. Error Model

Failures are returned as explicit, structured `Result` types:

* **`BuildError`:** Returned by `.build()` when a statement violates AST structural rules (such as unaliased duplicate table names or mismatched `DISTINCT ON` order prefixes).
* **`RowDecodeError`:** Returned during row extraction when a returned database value cannot be decoded into the expected Rust type.
* **`ExecutionError`:** Returned by terminal query calls, preserving the underlying driver error while providing classification helpers (`is_not_found`, `is_multiple_rows`, `is_row_decode`, `as_backend`).
