# Schema code generation

RURQE derives its schema markers from a live database catalog instead of from
hand-written declarations. The generator reads the catalog and emits a Rust
module declaring one marker per table and one marker per column, each carrying
the Rust type the column decodes into, so a query addresses a schema the
database actually has rather than a schema somebody remembered to update.

A normalized snapshot of the same catalog is written beside the generated code
as `schema.lock`, and the generated module embeds the digest of that snapshot as
`SCHEMA_HASH`. The two artifacts are therefore two views of one catalog, and a
module that no longer matches its snapshot is detectable without opening a
database.

This chapter covers what the catalog provides, how the generated module is
shaped, how the project configuration maps a database type onto a domain type of
the application, and what the snapshot records.

## What the catalog provides

The generator introspects one schema of the target database. The name comes from
`--schema`, otherwise from the `schema` key of the project configuration,
otherwise it is `public`. A SQLite database holds a single unnamed catalog, so
the name is ignored there and the whole file is described.

PostgreSQL:

* tables — every base table of the selected schema; a view is described on its own;
* columns — the declared type, the nullability, and the default expression. A column declared over a domain is described by the base type the domain wraps, and a chain of domains is followed to its end;
* enum types — every enum type of the schema together with its labels, in the order the engine sorts them;
* primary keys — the key columns in key order;
* foreign keys — the constrained columns, the referenced columns, and both referential actions;
* indexes — every secondary index with its uniqueness, its access method, its partial predicate, and the expressions it computes. Attributes an index only includes are not part of it, and the index a primary key builds is left out because the key already describes it;
* check constraints — the boolean expression each constraint enforces;
* views — the query defining each view;
* triggers — the declaration of every trigger defined by hand; a trigger an engine creates for a constraint is already described by the constraint.

SQLite:

* tables, views, and triggers — every object of the file together with the declaration it was created with;
* columns — the declared type, the nullability, the default value, and the position of the column inside the primary key. A declared type is kept verbatim and classified by affinity, so every integer column is described with the same integer type whatever its declaration spells;
* foreign keys — the constrained and referenced columns in key order, with both referential actions. A key that targets the implicit primary key of its parent table is resolved against the key columns of that table;
* indexes — every secondary index with its uniqueness, its indexed columns, the expressions it computes, and the predicate of a partial index;
* check constraints — every `CHECK` clause that is not an enum declaration;
* enums — a `CHECK (<column> IN (...))` clause closing the value set of a column becomes an enum type named after that column, and the column is typed as that enum. A column of the same name in another table that accepts a different set of values is distinguished as `<table>_<column>_enum`, so two value sets never collapse into one type.

The history table the migration engine keeps is not a relation of the application
schema, so it never reaches the generated module or the snapshot;
engine-internal relations of SQLite are skipped the same way.

The selection unit is the schema. There is no per-table include or exclude list,
because a table kept out of the generated module would also be kept out of the
snapshot, and the snapshot has to describe the schema a verification compares. A
relation that must not be generated is kept in a schema of its own, which the run
describes instead.

A selection that describes nothing is reported instead of silently producing an
empty catalog: when the selected schema holds no table and no view while the
database keeps relations in other schemas, the run states which schemas do hold
them, so an empty digest is never mistaken for a clean one. A SQLite file holds
one catalog, so the selection always describes it and no such notice arises.

## Running the generator

```bash
cargo db-codegen --database-url postgres://app@localhost:5432/app
```

| Option | Effect |
|---|---|
| `--database-url <url>` | The database to introspect. Falls back to the `DATABASE_URL` environment variable. |
| `--output <path>`, `-o <path>` | The file receiving the generated module. Defaults to `src/schema.rs`. |
| `--lock-path <path>` | The file receiving the snapshot. Defaults to `schema.lock`. |
| `--domain-types` | Maps the engine-neutral types onto their domain types. |
| `--schema <name>` | The schema to describe. Wins over the `schema` key of the configuration. |

Neither output path is configurable in the project file: both default and are
overridden by their own flags. The directory receiving an artifact is created if
it does not exist.

A project that exports `DATABASE_URL` and declares its schema and its overrides
in `rurqe.toml` regenerates both artifacts with the bare command:

```bash
cargo db-codegen
```

The run reports what it wrote:

```text
Generated schema to src/schema.rs and snapshot to schema.lock (hash 4f1d9c2e...)
```

Each artifact is written through a hidden sibling and renamed into place, and the
directory holding it is flushed before the run reports success, so a run that
fails while writing never truncates the file it was replacing.

The same command is reachable as `cargo rurqe codegen`.

### Connection target

The target is classified by the connection string: `postgres://` and
`postgresql://` URLs, and libpq keyword/value strings such as
`host=db.internal dbname=app`, address PostgreSQL, while a `sqlite:` URL,
`:memory:`, or a path ending in `.db`, `.sqlite`, or `.sqlite3` addresses SQLite.
A value that names neither is refused with both addressing forms stated in the
diagnostic instead of being guessed to be PostgreSQL.

PostgreSQL sessions of this build are cleartext, so the transport is policed in
front of the driver: a connection string demanding TLS (`require`, `verify-ca`,
`verify-full`) is refused on every host, and a host that is not loopback reached
without `sslmode=disable` is refused as a silent downgrade. A loopback address
and a Unix-domain socket are accepted in the modes that demand no TLS.
Terminating TLS in front of the server and acknowledging a local hop with
`sslmode=disable` are the two accepted arrangements; an acknowledged remote
cleartext session is reported as a warning once per process.

## The generated module

The module the generator writes is a schema declaration like a hand-written one:
the markers it declares are `table!` markers, and a query addresses them exactly
like markers written by hand. For a schema of one table and one enum it has this
shape:

```text
//! Automatically generated by RURQE. DO NOT EDIT DIRECTLY.
//!
//! To synchronize with the database catalog, run `cargo db-codegen`.

#![allow(dead_code)]

use rurqe::prelude::*;

pub const SCHEMA_HASH: &str = "4f1d9c2e...";

/// The `mood` enum.
#[derive(SqlType, Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mood {
    Happy,
}

table! {
    /// The `users` table.
    Users => "users" {
        UsersEmail => "email": String,
        UsersId => "id": i64,
        UsersMood => "mood": Option<Mood>,
    }
}

/// Strongly typed row record for `users`.
#[derive(Debug, Clone, PartialEq)]
pub struct UsersRecord {
    pub email: String,
    pub id: i64,
    pub mood: Option<Mood>,
}
```

The row decoder of the record is elided from the illustration: it reads the
fields positionally in the order they are declared and reports an `ArityMismatch`
when a row carries a different number of values, so a misaligned projection is a
typed error rather than an out-of-bounds read.

The module opens with an inner attribute, so it enters the crate as a module of
its own. A column is addressed through the table value (`Users.UsersId`) or
through its bare marker (`UsersId`), exactly as a hand-written declaration is
addressed. A variant whose database label the Rust identifier cannot spell keeps
the label through `#[sql(rename = "...")]`, so the value the database stores is
preserved exactly.

### Naming

* A table marker is the table name in PascalCase: `users` becomes `Users`, and `user.profiles` becomes `UserProfiles`. Characters outside `[A-Za-z0-9]` separate words, and a name beginning with a digit is prefixed with `_`.
* A column marker is the table marker followed by the column name in PascalCase: `users.id` becomes `UsersId`, and `2fa enabled` in the `user.profiles` table becomes `UserProfiles_2faEnabled`.
* A row record is the table marker followed by `Record`: `Users` becomes `UsersRecord`.
* A record field is the column name reduced to snake case: characters outside `[A-Za-z0-9]` become `_`, and a name beginning with a digit is prefixed with `_`. A Rust keyword is spelled as a raw identifier (`type` becomes `r#type`), and the keywords that cannot be raw identifiers (`crate`, `self`, `super`, `_`) are suffixed with `_` instead.

The generated items share one type namespace inside the module, so two schema
names that reduce to the same identifier are refused instead of rendering a
module with duplicate definitions:

```text
failed to generate schema code: schema names collide on the generated Rust
identifier `UsersRecord`: column `users.record`, record of table `users`; rename
them so they differ by more than characters that are invalid in Rust identifiers
```

Record fields need to be unique only within their own record, because each record
is a separate struct.

## Types

The Rust type of a column follows from the neutral type the catalog describes:

| Catalog type | Generated type | With domain types |
|---|---|---|
| `boolean` | `bool` | `bool` |
| `int32` | `i32` | `i32` |
| `int64` | `i64` | `i64` |
| `float64` | `f64` | `f64` |
| `numeric` | `String` | `rust_decimal::Decimal` |
| `text` | `String` | `String` |
| `bytes` | `Vec<u8>` | `Vec<u8>` |
| `uuid` | `String` | `uuid::Uuid` |
| `json` | `String` | `serde_json::Value` |
| `timestamp` | `String` | `chrono::NaiveDateTime` |
| `date` | `String` | `chrono::NaiveDate` |
| an enum type `mood` | `Mood` | `Mood` |
| an array of `text` | `Vec<String>` | `Vec<String>` |
| any other type | `String` | `String` |

An engine type outside the built-in set is preserved in the snapshot under its
declared spelling and generated as `String`, so an unfamiliar type never blocks a
regeneration.

A nullable column wraps the mapped type in `Option<...>`, and the wrapper is
applied once: an array wraps the vector rather than its elements, and a type
override that already spells an optional type is not wrapped a second time.

The default mapping keeps the generated module compilable with no dependency
beyond the library itself. Enabling domain types makes the module name the
ecosystem types directly, so the application enables the matching features
(`uuid`, `chrono`, `serde_json`, `decimal`) of the facade.

## Project configuration

The optional `rurqe.toml` is read from the working directory and carries the
schema a project describes and the per-column Rust types the generator emits:

```toml
schema = "app"

[codegen]
domain_types = true

[codegen.type_overrides]
"users.id" = "uuid::Uuid"
users.created_at = "chrono::NaiveDateTime"
```

An override may equally be nested one level deeper, which keeps a table with many
overrides readable:

```toml
[codegen.type_overrides.orders]
total = "rust_decimal::Decimal"
shipped_at = "chrono::NaiveDateTime"
```

The rules the file obeys:

* `schema` names the database schema the project's catalogs describe. The value is trimmed, a blank one is refused, and `--schema` wins over it.
* `codegen.domain_types` is a boolean. The `--domain-types` flag enables the same mapping, so a project states the choice once in the file and a one-off run enables it on the command line.
* Every override maps a `table.column` path onto the Rust type generated for that column. The quoted (`"users.id"`), dotted (`users.id`), and nested (`[codegen.type_overrides.users]`) spellings name the same entry.
* The value is emitted verbatim as the column type, so it must be a syntactically valid Rust type. An override survives every later regeneration, because the generator never edits the configuration.
* The nullability of the column still applies: a nullable column wraps the override in `Option<...>` unless the value already spells an optional type. The value is parsed as a Rust type to decide, so `Option<T>`, `std::option::Option<T>`, and `core::option::Option<T>` are all recognised.
* A value that is not a string, an entry whose path lacks one of its two components, an empty type, and a repeated declaration of the same path are refused. The quoted and the dotted spelling of one path collide even though TOML treats them as distinct keys.
* A key the format does not define is refused at the root and inside the `codegen` section alike, and the diagnostic names the line and the column of the declared key rather than a guessed offset.

The two diagnostics a misspelled file produces:

```text
invalid configuration at line 2, column 1: unknown configuration section `workspace`
```

```text
invalid configuration at line 3, column 1: duplicate type override `users.id`
```

A file that is malformed in any of these ways aborts the run instead of being
partially applied: a misspelled key would generate a module with the wrong types,
and a misspelled schema would describe a catalog the project never targeted. A
project without the file keeps the defaults, and every command that compares
catalogs resolves the schema name the same way.

## The snapshot

The snapshot is a versioned TOML document describing the same catalog as the
generated module:

| Section | Content |
|---|---|
| `version` | The format version, `5`.
| `[[enums]]` | One enum type with its labels, in declaration order. |
| `[[tables]]` | One table with its columns, primary key, check constraints, foreign keys, and indexes. |
| `[[views]]` | One view with the query defining it. |
| `[[triggers]]` | One trigger with the table it is attached to and its definition. |

A column entry carries the name, the neutral type, the nullability, and — when
the column declares one — the default expression verbatim. An enum type is
spelled as a named variant, an array as its element type, and an engine type
outside the built-in set as its declared spelling:

```toml
version = 5

[[enums]]
name = "mood"
variants = ["happy", "sad"]

[[tables]]
name = "users"
columns = [
  { name = "email", type = "text", nullable = false },
  { name = "id", type = "int64", nullable = false },
  { name = "mood", type = { enum = { name = "mood" } }, nullable = true },
  { name = "tags", type = { array = { element = "text" } }, nullable = false, default = "'{}'" },
]
primary_key = ["id"]
checks = []
foreign_keys = []
indexes = []
```

Everything the snapshot lists is ordered by name — enums, tables, views,
triggers, and inside a table its columns, checks, foreign keys, and indexes — so
the document depends only on the schema content and never on the order a catalog
reported it in. The orders that carry meaning are kept: the labels of an enum
stand in declaration order, and the columns of a primary key, of a foreign key,
and of an index remain in the order they were declared.

A snapshot written by an earlier release — the previous TOML version, or one that
opens with the header of the older text format — is still read, so upgrading the
toolchain does not invalidate a committed snapshot. A snapshot of the current
format is read strictly: a key the format does not define and a version the reader
does not know are both refused, so a snapshot is never read halfway.

## The schema digest

`SCHEMA_HASH` is the SHA-256 digest of the snapshot text. The generated module
embeds it, so the module states which snapshot it was generated from: a module
regenerated after a schema change carries a different digest, and a module whose
digest no longer matches the committed snapshot is refused by verification
instead of being trusted.

The generated module and the snapshot close the loop the rest of the lifecycle
depends on. The catalog a database holds is described once, and every later
comparison — the digest embedded in the code, the snapshot committed with the
project, the catalog a database reports — is made against these two artifacts.
