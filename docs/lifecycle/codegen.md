# Schema code generation

RURQE derives its schema markers from a live database catalog instead of from
hand-written declarations. The generator reads the catalog and emits a Rust
module declaring one marker per table and one marker per column, each carrying
the Rust type the column decodes into, so a query addresses a schema the
database actually has rather than a schema somebody remembered to update.

A normalised snapshot of the same catalog is written into the artifact directory
as `rurqe/schema.lock`, and the generated module embeds the digest of that catalog
as `SCHEMA_HASH`. The two artifacts are therefore two views of one catalog, and a
module that no longer matches its snapshot is detectable without opening a
database.

This chapter covers what the catalog provides, how the generated module is
shaped, how the project configuration maps a database type onto a domain type of
the application, and what the snapshot records.

## What the catalog provides

The generator introspects the schemas a project declares: one primary schema,
and any number of secondary schemas beside it. The primary schema comes from
`--schema`, otherwise from the `primary_schema` key of the project configuration
(or its legacy `schema` alias), otherwise it is `public`; the secondary schemas
come from the `schemas` array of the same file. Every entity the catalog reports
carries the schema it belongs to, so a project declaring several schemas
describes all of them in one snapshot. A SQLite database holds a single unnamed
catalog, so the names are ignored there and the whole file is described.

PostgreSQL:

* tables — every base table of the selected schemas; a view is described on its own;
* columns — the declared type, the nullability, and the default expression. A column declared over a domain is described by the base type the domain wraps, and a chain of domains is followed to its end;
* enum types — every enum type of the selected schemas together with its labels, in the order the engine sorts them;
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
Every object of both lists is attributed to the schema it belongs to, so two
relations of one name in two schemas stay distinguishable in the snapshot and in
the generated module.

The history table the migration engine keeps is not a relation of the application
schema, so it never reaches the generated module or the snapshot;
engine-internal relations of SQLite are skipped the same way.

The selection unit is the schema. There is no per-table include or exclude list,
because a table kept out of the generated module would also be kept out of the
snapshot, and the snapshot has to describe the schemas a verification compares. A
relation that must not be generated is kept in a schema of its own, which the run
does not select.

A selection that describes nothing is reported instead of silently producing an
empty catalog: when none of the selected schemas holds a table or a view while
the database keeps relations in other schemas, the run states the whole selection
and names the schemas that do hold them, so an empty digest is never mistaken for
a clean one. The notice points at `--schema` for the primary schema and at the
`primary_schema` key of the configuration file, while the remaining schemas are
declared in `schemas`. A SQLite file holds one catalog, so the selection always
describes it and no such notice arises.

## Running the generator

```bash
cargo rurqe codegen --database-url postgres://app@localhost:5432/app
```

| Option | Effect |
|---|---|
| `--database-url <url>` | The database to introspect. Falls back to the `DATABASE_URL` environment variable. |
| `--domain-types` | Maps the engine-neutral types onto their domain types. |
| `--schema <name>` | The primary schema to describe. Wins over the `primary_schema` key of the configuration, and the secondary schemas come from `schemas`. |
| `--config <path>` | The configuration file whose keys resolve the paths of the invocation. Defaults to the `rurqe.toml` of the working directory. |

The generated module is resolved through `[codegen].output`, and the built-in
`src/schema.rs` closes the cascade when the key names no file. The snapshot carries
no key of the document, so it lives at the conventional `rurqe/schema.lock` beneath
the artifact directory. The directory receiving an artifact is created if it does not
exist.

A project that exports `DATABASE_URL` and declares its schema, its module path,
and its overrides in `rurqe.toml` regenerates both artifacts with the bare
command:

```bash
cargo rurqe codegen
```

The run reports what it wrote:

```text
Generated schema to src/schema.rs and snapshot to rurqe/schema.lock (hash 4f1d9c2e...)
```

Each artifact is written through a hidden sibling and renamed into place, and the
directory holding it is flushed before the run reports success, so a run that
fails while writing never truncates the file it was replacing.

The snapshot is written by the same run as the module, so the two artifacts
never describe different catalogs.

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
like markers written by hand. The sample below opens the module the generator
emitted for a catalog of one table over one enum in the primary schema, with the
domain types enabled. The book regenerates that module from the same catalog and
compiles it, so the header, the names, and the row decoder shown are the ones the
generator writes:

```rust,no_run
{{#rustdoc_include ../examples/generated/schema_module.rs:flat}}
```

The row decoder reads the fields positionally in the order they are declared and
reports an `ArityMismatch` when a row carries a different number of values, so a
misaligned projection is a typed error rather than an out-of-bounds read.

The module opens with an inner attribute, so it enters the crate as a module of
its own. A column is addressed through the table value (`Users.UsersId`) or
through its bare marker (`UsersId`), exactly as a hand-written declaration is
addressed. A variant whose database label the Rust identifier cannot spell keeps
the label through `#[sql(rename = "...")]`, so the value the database stores is
preserved exactly.
### Tables of a secondary schema

The entities of the primary schema are rendered flat at the top level of the
module, which is what lets a single-schema project address them through the bare
markers. The entities of a secondary schema are grouped into a `schema!` block:
the block states the SQL name of the schema once, declares the marker the schema
is addressed through, and keeps the markers of its tables inside the module of
that schema.

The catalog that module was generated from declares a second schema, `auth`,
with one table in it, and for that table the block has this shape:

```rust,no_run
{{#rustdoc_include ../examples/generated/schema_module.rs:secondary}}
```

A table of the primary schema keeps rendering its bare name (`FROM "users"`),
while a table of a secondary schema renders its qualifier
(`FROM "auth"."sessions"`). The query surface is otherwise identical: a column
of a secondary schema is addressed as `Auth.Sessions.SessionsId` as a value, and
as `auth::SessionsId` where a signature has to name the type.

### Naming

* A table marker is the table name in PascalCase: `users` becomes `Users`, and `user.profiles` becomes `UserProfiles`. Characters outside `[A-Za-z0-9]` separate words, and a name beginning with a digit is prefixed with `_`.
* A column marker is the table marker followed by the column name in PascalCase: `users.id` becomes `UsersId`, and `2fa enabled` in the `user.profiles` table becomes `UserProfiles_2faEnabled`.
* A row record is the table marker followed by `Record`: `Users` becomes `UsersRecord`.
* A record field is the column name reduced to snake case: characters outside `[A-Za-z0-9]` become `_`, and a name beginning with a digit is prefixed with `_`. A Rust keyword is spelled as a raw identifier (`type` becomes `r#type`), and the keywords that cannot be raw identifiers (`crate`, `self`, `super`, `_`) are suffixed with `_` instead.
* A schema marker is the schema name in PascalCase: `auth` becomes `Auth`, and `user.profiles` becomes `UserProfiles`. The module of a schema is the same name reduced to a snake-case identifier: `auth` stays `auth`, `user.profiles` becomes `user_profiles`, and a keyword keeps its raw spelling.

The generated items share one type namespace inside the module, so two schema
names that reduce to the same identifier are refused instead of rendering a
module with duplicate definitions:

```text
failed to generate schema code: schema names collide on the generated Rust
identifier `UsersRecord`: column `users.record`, record of table `users`; rename
them so they differ by more than characters that are invalid in Rust identifiers
```
A schema marker and its module are claimed in the top-level namespace beside the
flat items of the primary schema and beside every other schema marker, so a
schema named after a primary table is refused the same way:

```text
failed to generate schema code: schema names collide on the generated Rust
identifier `Auth`: table `auth`, schema `auth`; rename them so they differ by
more than characters that are invalid in Rust identifiers
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

The optional `rurqe.toml` is read from the working directory when no `--config`
names another file, and it carries the schemas a project describes and the
per-column Rust types the generator emits:

```toml
primary_schema = "app"
schemas = ["auth", "billing"]

[codegen]
domain_types = true

[codegen.type_overrides]
"users.id" = "uuid::Uuid"
users.created_at = "chrono::NaiveDateTime"
"wallets.address" = "bitcoin::Address"
"transactions.txid" = "bitcoin::Txid"
"auth.users.id" = "uuid::Uuid"
```

An override may equally be nested one level deeper, which keeps a table with many
overrides readable:

```toml
[codegen.type_overrides.orders]
total = "rust_decimal::Decimal"
shipped_at = "chrono::NaiveDateTime"

[codegen.type_overrides.auth.users]
id = "uuid::Uuid"
```

The first table belongs to the primary schema, so its keys name the column under
the table it lives in. The second belongs to a secondary schema, so the schema
comes first and the path carries three segments.

An override is what keeps a domain newtype in place: a binary column would
otherwise be generated as `Vec<u8>` and a text column as `String`, so
`bitcoin::Txid` and `bitcoin::Address` name the types the application really
binds, and the newtype declares its conversions once through
`impl_sql_binary_newtype!` or `#[derive(SqlType)]`.

The rules the file obeys:

* `primary_schema` names the database schema the project's catalogs describe — the one its relations render without a qualifier. The value is trimmed, a blank one is refused, and `--schema` wins over it.
* `schemas` names the secondary schemas declared beside the primary one. An entry is trimmed, a blank one is refused, a name may appear once, and no entry may repeat the primary schema — the declared one or the `public` default — because the selection would then describe the same catalog twice.
* `schema` is the legacy spelling of `primary_schema` and names the same setting. Declaring both is refused, with the position of each key in the diagnostic, so one file never selects two primary schemas.
* `codegen.domain_types` is a boolean. The `--domain-types` flag enables the same mapping, so a project states the choice once in the file and a one-off run enables it on the command line.
* `codegen.output` names the generated module `codegen` writes and `verify` reads, and `src/schema.rs` closes the cascade when the key names no file.
* The value is emitted verbatim as the column type, so it must be a syntactically valid Rust type. An override survives every later regeneration, because the generator never edits the configuration.
* The nullability of the column still applies: a nullable column wraps the override in `Option<...>` unless the value already spells an optional type. The value is parsed as a Rust type to decide, so `Option<T>`, `std::option::Option<T>`, and `core::option::Option<T>` are all recognised.
* A value that is not a string, a path that is neither `table.column` nor `schema.table.column` (an empty segment included), an empty type, and a repeated declaration of the same path are refused. The quoted and the dotted spelling of one path collide even though TOML treats them as distinct keys.
* A key the format does not define is refused at the root and inside the `codegen` section alike, and the diagnostic names the line and the column of the declared key rather than a guessed offset.

The diagnostics a misspelled file produces:

```text
invalid configuration at line 2, column 1: unknown configuration section `workspace`
```

```text
invalid configuration at line 3, column 1: duplicate type override `users.id`
```

```text
invalid configuration at line 3, column 1: `schema` and `primary_schema` both configure the primary schema; keep `primary_schema` (line 1, column 1) and remove the legacy `schema` key
```

A file that is malformed in any of these ways aborts the run instead of being
partially applied: a misspelled key would generate a module with the wrong types,
and a misspelled schema would describe a catalog the project never targeted. A
project without the file keeps the defaults, and every command that compares
catalogs resolves the schema selection the same way.

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

`SCHEMA_HASH` is the SHA-256 digest of the catalog, not of the text that
describes it. The catalog is encoded into a canonical byte stream — a versioned
document whose fields are unambiguously delimited and whose entities are ordered
by the schema content alone — and the digest is taken over that stream, so the
line breaking, the inline-table layout, and the string escaping of the snapshot
take no part in the identity it carries.

The generated module embeds the digest, so the module states which catalog it was
generated from: a module regenerated after a schema change carries a different
digest, and a module whose digest no longer matches the committed snapshot is
refused by verification instead of being trusted.

The module sample in this chapter spells the digest as `<SCHEMA_HASH>`: the
digest is a value the catalog computes, so the text carries a placeholder where
the generator writes the digest, and the book substitutes the value the catalog
reports when it regenerates the sample.

The generated module and the snapshot close the loop the rest of the lifecycle
depends on. The catalog a database holds is described once, and every later
comparison — the digest embedded in the code, the snapshot committed with the
project, the catalog a database reports — is made against these two artifacts.
