# Database Schema

A schema in RURQE is defined using zero-sized marker types (`ZST`). Table and column markers carry no runtime data; they encode table identifiers, column names, nullability invariants, and Rust types directly at compile time.

## Declaring Tables and Columns

The `table!` macro declares zero-sized table and column markers:

```rust,no_run
{{#rustdoc_include ../examples/schema.rs:declaration}}
```

The macro generates a unit struct for the table and one unit struct per column marker. Because unit structs serve simultaneously as types and as values in Rust, a column marker is passed directly as a value in a predicate — no turbofish generic syntax, no wrapper expression — as the addressing form written below shows.

## Value-Based Column Addressing

RURQE supports two addressing styles for table columns:

* **Standalone Column Markers (`UserId`):** Used in queries referencing a single relation, minimizing syntactic noise.
* **Table-Prefixed Columns (`User.UserId`):** Used when disambiguating columns across multiple joined tables or aliases.

Both forms refer to the identical zero-sized marker, generate identical SQL AST nodes, and compile with zero runtime overhead:

```rust,no_run
{{#rustdoc_include ../examples/schema.rs:addressing}}
```
## Grouping Tables by Schema

A database keeps its relations in schemas, and the DSL mirrors that structure. The `schema!` macro declares one schema together with the tables it contains:

```rust,no_run
{{#rustdoc_include ../examples/schema.rs:schema_declaration}}
```

The declaration states the SQL name of the schema once and forwards every table to `table!` with that name attached, so the tables and the columns keep the shape a plain declaration produces. The identifier after `as` is the module holding the type namespace: a declarative macro cannot change the case of an identifier, so the marker stays in PascalCase (`Auth`) while the module follows the naming convention of modules (`auth`).

The macro occupies both namespaces, exactly as a table declaration does:

* the module `auth` carries the table and column markers as types, so a signature spells `auth::Sessions` or `auth::SessionsId`;
* the struct `Auth` carries one field per table, and the constant `Auth` of that type carries the same markers as values, so a column is addressed as `Auth.Sessions.SessionsId`.

Items that belong next to the tables — a record, an enumeration, a table alias — are declared in a trailing `@items` block, which is written into the module verbatim.

A table declared outside a schema block states the schema it lives in with the `in` clause:

```rust,no_run
{{#rustdoc_include ../examples/schema.rs:table_in_clause}}
```

## Primary and Secondary Schemas

A project names one schema as its primary schema, and every other schema a table belongs to is a secondary schema. The distinction is visible in the SQL a relation produces:

| Declaration | Rendered relation |
|---|---|
| `User => "users"` — a table of the primary schema | `"users"` |
| `Sessions => "sessions" in "auth"` — a table of a secondary schema | `"auth"."sessions"` |

A table of the primary schema renders its bare name, so a statement over it reads exactly as it would without the schema layer. A table of a secondary schema renders its qualifier, which is what keeps two relations of one name in two schemas apart. A column reference stays qualified by the table name or by an alias alone: the relation reference is already unambiguous, so no column is spelled with its schema.

## Querying a Table of a Secondary Schema

A schema value addresses the tables it declares, and a table value addresses its columns, so a secondary relation is named the same way a primary one is — no path, no turbofish:

```rust,no_run
{{#rustdoc_include ../examples/schema.rs:schema_addressing}}
```

The statement the fragment assembles:

```sql
SELECT "id" AS "sessions_id" FROM "auth"."sessions" WHERE ("id" = $1)
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

* `cargo db-codegen`: Introspects tables, views, primary keys, foreign keys, enums, and nullability, outputting a typed `schema.rs` and writing the normalized catalog snapshot to `schema.lock`.
* `cargo db-verify`: Verifies in CI that migrations, the database catalog, and the generated schema code remain completely synchronized.

In a real project the markers are not written by hand: `cargo db-codegen` introspects the live database catalog and generates a `schema.rs` with the same unit structs, the `SCHEMA_HASH` constant, and DTO structs. The catalog snapshot is fixed in `schema.lock`, and `cargo db-verify` in CI checks that the code, the migrations, and the database have not drifted apart. The snapshot and the generated module cover every schema the project declares: the relations of the primary schema stay flat items of the module, while the relations of every secondary schema are grouped into a `schema!` block. A hand-written `table!` remains the tool for tests and small projects, and the examples in this book use it.
