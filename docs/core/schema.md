# Database Schema

A schema in RURQE is defined using zero-sized marker types (`ZST`). Table and column markers carry no runtime data; they encode table identifiers, column names, nullability invariants, and Rust types directly at compile time.

## Declaring Tables and Columns

The `table!` macro declares zero-sized table and column markers:

```rust,no_run
{{#rustdoc_include ../examples/schema.rs:declaration}}
```

Each invocation declares exactly one table and ends with a semicolon, so a declaration is a single statement rather than the opening of a block that could hold several. The macro generates a unit struct for the table and one unit struct per column marker. Because unit structs serve simultaneously as types and as values in Rust, a column marker is passed directly as a value in a predicate — no turbofish generic syntax, no wrapper expression — as the addressing form written below shows.

## Value-Based Column Addressing

RURQE supports two addressing styles for table columns:

* **Standalone Column Markers (`UserId`):** Used in queries referencing a single relation, minimising syntactic noise.
* **Table-Prefixed Columns (`User.UserId`):** Used when disambiguating columns across multiple joined tables or aliases.

Both forms refer to the identical zero-sized marker, generate identical SQL AST nodes, and compile with zero runtime overhead:

```rust,no_run
{{#rustdoc_include ../examples/schema.rs:addressing}}
```
## Rust Type to SQL Type Mapping

Column types declared in `table!` map directly to database types and SQL `CAST` representations:

| Rust type | Standard SQL type |
|---|---|
| `i16` | `SMALLINT` |
| `i32` | `INTEGER` |
| `i64` | `BIGINT` |
| `f64` | `DOUBLE PRECISION` |
| `bool` | `BOOLEAN` |
| `String` | `TEXT` |
| `Vec<u8>` | `BYTEA` / `BLOB` |
| `uuid::Uuid` (feature `uuid`) | `UUID` |
| `chrono::NaiveDateTime` (feature `chrono`) | `TIMESTAMP` |
| `chrono::DateTime<Utc>` (feature `chrono`) | `TIMESTAMPTZ` |
| `chrono::NaiveDate` (feature `chrono`) | `DATE` |
| `rust_decimal::Decimal` (feature `decimal`) | `NUMERIC` |
| `serde_json::Value` / `Json<T>` (feature `serde_json`) | `JSON` / `JSONB` |

Wrapping a type in `Option<T>` marks the column as nullable in the schema. In addition, custom domain newtypes decorated with `#[derive(SqlType)]` automatically forward their underlying representation.

## Single Source of Truth: Automated Codegen

While `table!` is ideal for tests, rapid prototyping, and self-contained examples, production applications generate schema definitions directly from the live database catalog via `rurqe-cli`:

* `cargo rurqe codegen`: Introspects tables, views, primary keys, foreign keys, enums, and nullability, outputting a typed `schema.rs` and writing the normalised catalog snapshot to `schema.lock`.
* `cargo rurqe verify`: Verifies in CI that migrations, the database catalog, and the generated schema code remain completely synchronised.

In a real project the markers are not written by hand: `cargo rurqe codegen` introspects the live database catalog and generates a `schema.rs` with the same unit structs, the `SCHEMA_HASH` constant, and DTO structs. The catalog snapshot is fixed in `schema.lock`, and `cargo rurqe verify` in CI checks that the code, the migrations, and the database have not drifted apart. The snapshot and the generated module cover every schema the project declares: the relations of the primary schema stay flat items of the module, while the relations of every secondary schema are grouped into a `schema!` block. A hand-written `table!` remains the tool for tests and small projects, and the examples in this book use it.
