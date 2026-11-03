# Quickstart

This chapter walks the workflow of an application built on RURQE: it declares what the toolchain has to know about the project, generates the schema markers from the live database catalog, writes a typed query over those markers, and executes it against a connection pool.

Query building performs no I/O, so every statement below compiles without a reachable database. The generation step and the terminal call of the query are the only places that open a connection.

## 1. Dependencies and Cargo features

Add `rurqe` to `Cargo.toml` with the dialect and execution features the project targets:

```toml
[dependencies]
rurqe = { version = "0.1", features = ["postgres", "tokio-postgres", "deadpool-postgres", "macros"] }
# for SQLite: features = ["sqlite", "macros"]
```

| Feature | Functionality |
|---|---|
| `sqlite` | Synchronous SQLite driver (`SqliteConnection`), version markers `Sqlite3_0` through `Sqlite3_38` |
| `postgres` | Synchronous PostgreSQL driver (`PgConnection`) based on `postgres` |
| `tokio-postgres` | Asynchronous PostgreSQL connection adapter (`PgAsyncConnection`) |
| `deadpool-postgres` | Asynchronous PostgreSQL connection pool (`PgAsyncPool`) |
| `r2d2` | Synchronous PostgreSQL connection pool (`PgPool`) and SQLite connection pool (`SqlitePool`) |
| `macros` | Procedural and declarative macros: `select!`, `cols!`, `dto!`, `sql!`, `raw_sql!`, `#[derive(SqlType)]` |
| `migrate` | Embedded migration engine (`EmbeddedMigrations`, `embed_migrations!`); it also enables the `migrate` feature of every driver in the build, so a baked collection applies over a connection the application already owns |
| `uuid`, `chrono`, `serde_json`, `decimal` | Ecosystem data type mappings |

## 2. Project configuration

The toolchain reads its settings from `rurqe.toml` in the working directory. A minimal file states the schemas the catalog digests describe and the module code generation writes:

```toml
primary_schema = "app"
schemas = ["auth"]

[codegen]
output = "src/schema.rs"
```

* `primary_schema` names the database schema the project's catalogs describe — the one its relations render without a qualifier.
* `schemas` names the secondary schemas declared beside the primary one.
* `codegen.output` names the module the generator writes and the drift gate reads, and the built-in `src/schema.rs` closes the cascade when the key is absent.

Every declared path resolves against the directory holding the configuration file, never against the working directory of a command. The file is optional: a project whose relations live in `public` and whose module sits at `src/schema.rs` omits it and keeps the defaults. It carries no connection setting either, so the target of a command is always taken from its arguments or from the environment.

## 3. Schema code generation

```bash
cargo rurqe codegen --database-url postgres://app@localhost:5432/app
```

The command introspects the declared schemas and writes two artifacts from one catalog:

* the marker module at `src/schema.rs` — one marker per table and one marker per column, each carrying the Rust type the column decodes into, so a query addresses a relation the database actually has;
* the snapshot `rurqe/schema.lock` beneath the artifact directory, which is the normalised catalog the lifecycle compares against.

The module embeds the digest of the catalog as `SCHEMA_HASH`, so the code and the snapshot it was generated from stay provably in step, and a module that no longer matches its snapshot is detectable without opening a database. For a `users` table of three columns the module declares the table, its columns, and the digest it was generated from:

```rust,no_run
{{#rustdoc_include examples/quickstart.rs:schema}}
```

A project ships that file as `src/schema.rs` and declares `mod schema;` in its crate root, then imports the markers it queries. The example below keeps the module beside the code that addresses it, because an example target is a single file. The generator also emits a row record per table beside the markers, which a query that names its own projection does not need.

The database is named by `--database-url`, or by the `DATABASE_URL` environment variable when the flag is absent, so a project that exports the variable regenerates both artifacts with the bare `cargo rurqe codegen`. Both engines are addressed the same way: a `postgres://` or `postgresql://` URL and a libpq keyword string address PostgreSQL, while a `sqlite:` URL, `:memory:`, or a path ending in `.db`, `.sqlite`, or `.sqlite3` addresses SQLite.

## 4. Typed queries

The generated module is a schema declaration like a hand-written one: a table and each of its columns are values, so a query names a relation or a column directly and the chain carries no generic annotation.

```rust,no_run
{{#rustdoc_include examples/quickstart.rs:query}}
```

Assembling the chain opens no connection. Execution happens at the terminal call alone (`.fetch_all(&pool).await?`), which decodes every row into a synthesised record; the chain that builds the statement is the same for the blocking drivers, and only the terminal call differs.

A projection names its items, so a fetched row is read by those names rather than by a position in the row:

```rust,no_run
{{#rustdoc_include examples/quickstart.rs:named_rows}}
```

```sql
SELECT "id" AS "user_id", "name" AS "contact_email", "email" AS "user_email"
FROM "users"
WHERE ("email" = $1)
ORDER BY "id" ASC
```

## 5. Error model

Failures are returned as explicit, structured `Result` types:

* **`BuildError`:** Returned by `.build()` when a statement violates a structural rule of the SQL AST, such as a relation addressed twice under one name or a `DISTINCT ON` prefix that disagrees with the leading ordering criterion.
* **`RowDecodeError`:** Returned during row extraction when a returned database value cannot be decoded into the expected Rust type.
* **`ExecutionError`:** Returned by terminal query calls, preserving the underlying driver error while providing classification helpers (`is_not_found`, `is_multiple_rows`, `is_row_decode`, `as_backend`).

A failure travels in the type that produced it, so a terminal call is propagated with `?` and the typed error surfaces at the runtime boundary of the application.

## 6. Hand-written schema declarations

Schema code generation is the primary route, because it keeps the markers, the snapshot, and the database catalog in step. A hand-written `table!` declaration stays available as a low-level fallback for isolated unit tests and rapid prototypes, where the crate is built with no reachable database:

```rust,no_run
{{#rustdoc_include examples/quickstart.rs:fallback}}
```

The generated module declares its markers the same way, so a relation and its columns are values under both forms, and the builder calls a test exercises are the calls the generated module receives. A hand-written declaration carries no snapshot and no `SCHEMA_HASH`, so it never stands in for the generated module when drift is verified.
