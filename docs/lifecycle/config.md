# Project configuration

`rurqe.toml` is the optional file that states what the toolchain has to know about a project before it opens a database: the directory its lifecycle artifacts live under, the schemas its catalog digests describe, and the Rust type the generator emits for a column whose engine type maps onto a domain type of the application.

## Choosing the file

Two things decide which file a command reads.

- `--config <path>` names it explicitly, and every command of the toolchain accepts the flag: `migrate up`, `migrate status`, `migrate repair`, `baseline`, `codegen`, `verify`, `stage init`, `stage verify`, `stage commit`, `stage cleanup`, and `prune`.
- Without the flag, the conventional `rurqe.toml` of the working directory is read when it exists.

The two tiers are deliberately asymmetric, because a path the operator named is a path the operator meant:

```text
configuration file `config/rurqe.toml` named by `--config` does not exist; a path the flag names is required, while the conventional `rurqe.toml` is optional
```

```text
configuration file `config` named by `--config` is not a regular file
```

The conventional file stays optional, and a project without one keeps the defaults: no schema is named, so `public` applies, the secondary list is empty, no column carries an override, every artifact sits beneath the directory `rurqe`, and the generated module carries no domain types.

Every declared path resolves against the directory holding the file, never against the working directory of the command, so `--config db/production.toml` reads and writes the artifacts of `db/` wherever the command was started. The conventional file of the working directory resolves them there, which is the same rule with the current directory as its anchor.

The document is read strictly, because a misspelled key would resolve an artifact into a directory the project never intended, generate a module with the wrong types, or describe a catalog the project never targeted. An unknown key, a value of the wrong shape, a blank name, and a repeated declaration are each refused with the line and the column of the declaration that caused it, and a file that is malformed in any way aborts the command instead of being partially applied — a command that consumes no artifact path at all, `stage cleanup` included, validates the file it accepted just the same.

| Key | Type | Default | Purpose |
|---|---|---|---|
| `root` | string | `rurqe` | The artifact directory every lifecycle artifact lives beneath. |
| `primary_schema` | string | `public` | The database schema the project's catalog digests describe. |
| `schema` | string | — | The legacy spelling of `primary_schema`; both keys configure one setting. |
| `schemas` | array of strings | `[]` | The secondary schemas described beside the primary one. |
| `migrations.dir` | string | `<root>/migrations` | The directory holding the migration tail. |
| `stage.dir` | string | `<root>/schema` | The directory holding the declarative stage. |
| `stage.checkpoint` | string | `<migrations.dir>/checkpoint.toml` | The file holding the boundary marker. |
| `codegen.output` | string | `src/schema.rs` | The module code generation writes and verification reads. |
| `codegen.domain_types` | boolean | `false` | Whether the generator emits the application's domain types. |
| `codegen.type_overrides` | table | empty table | The Rust type generated for a named column. |

Nothing else is accepted, at the root and inside the sections alike, and the refusal lands on the declaration:

```text
invalid configuration at line 1, column 1: unknown configuration section `primary_schemas`
```

```text
invalid configuration at line 6, column 1: unknown key `record_format` in the `codegen` section
```

The wording of the first refusal covers every undeclared root key, a near-miss of a real one included. A section that is not a table is refused the same way:

```text
invalid configuration at line 4, column 1: `codegen` must be a table
```

The generator's own defaults — the module doc comment and the row records — are not settings of this file: the file parameterises paths and types, not the shape of what is rendered.

## A complete file

```toml
root = "rurqe"
primary_schema = "app"
schemas = ["auth", "billing"]

[migrations]
dir = "rurqe/migrations"

[stage]
dir = "rurqe/schema"
checkpoint = "rurqe/migrations/checkpoint.toml"

[codegen]
output = "src/db/schema.rs"
domain_types = true

[codegen.type_overrides]
"users.id" = "uuid::Uuid"
"users.created_at" = "chrono::NaiveDateTime"
"wallets.address" = "bitcoin::Address"
"transactions.txid" = "bitcoin::Txid"
```

Every path in it spells a conventional one except the generated module, so the file states the whole arrangement in one place and a project that relocates one artifact states only the key that moved.

## The artifact directory

```toml
root = "artifacts"
```

`root` names the directory every lifecycle artifact lives beneath, `rurqe` by default. One key relocates the whole layout: the declarative stage, the migration tail, the boundary marker, the boundary snapshot, and the catalog snapshot all follow it, and no project restates them path by path.

```text
project/
├── rurqe.toml                  the configuration itself
├── rurqe/                      the artifact directory
│   ├── schema/                 declarative stage: the target DDL
│   ├── migrations/             active migration tail
│   │   ├── checkpoint.toml     boundary marker
│   │   └── checkpoint-stage/   boundary snapshot, named after the marker
│   └── schema.lock             catalog snapshot of the committed stage
└── src/
    └── schema.rs               generated module, `src/schema.rs` by default
```

The value is a string and is trimmed, and a blank one is refused:

```text
invalid configuration at line 1, column 1: `root` requires a non-empty value
```

A value that is not a string at all is refused in its place, with `` `root` must be a string ``.

## The migration tail

```toml
[migrations]
dir = "rurqe/migrations"
```

`dir` names the directory the migration scripts are read from. The value resolves against the directory of the configuration file, and `rurqe/migrations` beneath that directory when the key is absent.

## The declarative stage

```toml
[stage]
dir = "rurqe/schema"
checkpoint = "rurqe/migrations/checkpoint.toml"
```

`dir` names the directory holding the declarative DDL of the target schema, `rurqe/schema` by default.

`checkpoint` names the boundary marker. When the key is absent the marker is `checkpoint.toml` inside the resolved migration directory, so relocating the tail relocates its marker with it. The boundary snapshot is named after the stem of the marker and sits beside it: `checkpoint.toml` describes `checkpoint-stage/`, and `prod.toml` describes `prod-stage/`.

Both keys are strings and are trimmed, and a blank one is refused:

```text
invalid configuration at line 1, column 1: `stage.checkpoint` requires a non-empty value
```

## The path cascade

Every path of a command is resolved through the same two tiers: the key of the
configuration file, and the conventional path beneath the artifact directory. The
command line carries no path option of its own, so a project states where its
artifacts live once, in the file, and an invocation that has to read another layout
names another configuration file with `--config`.

| Path | Key | Conventional |
|---|---|---|
| Migration tail | `migrations.dir` | `rurqe/migrations` |
| Declarative stage | `stage.dir` | `rurqe/schema` |
| Boundary marker | `stage.checkpoint` | `rurqe/migrations/checkpoint.toml` |
| Catalog snapshot | — | `rurqe/schema.lock` |
| Generated module | `codegen.output` | `src/schema.rs` |

`codegen` writes and `verify` reads the same module, so the two share one key:
`codegen.output` names it for both commands. The last tier differs on purpose —
`codegen` falls back to `src/schema.rs` and writes it, while `verify` treats the
conventional module as optional and falls back to the snapshot alone when the key
names no file.

One consequence is worth stating on its own: the marker a guard reads is the marker
every other command reads. The guard that refuses a target older than the pruned
boundary resolves the migration tail and the boundary marker from the project
configuration alone, so a run can never consult a marker that no other command of
the project would ever read, and no invocation can arm one command with a boundary
the rest of the lifecycle never sees.


## Schema selection

A command that compares catalogs has to know which schemas it describes. The file names them: one primary schema, and any number of secondary schemas beside it.

### `primary_schema`

```toml
primary_schema = "app"
```

Names the database schema the project's catalogs describe — the one its relations render without a qualifier, so a table reached as `FROM "users"` lives in it. The value is a string and is trimmed, and a blank one is refused:

```text
invalid configuration at line 1, column 1: `primary_schema` requires a database schema name
```

A value that is not a string at all is refused in its place, with `` `primary_schema` must be a database schema name string ``.

### The legacy `schema` key

`schema` names the same setting under its older spelling, so a project that already declares it keeps working without an edit. The two keys are one setting rather than two, and declaring both is refused instead of ranked: the file would otherwise name two primary schemas without saying which one wins. The refusal sits on the legacy key — the declaration whose removal remedies the file — and it names the position of `primary_schema`, so both are located at once:

```text
invalid configuration at line 2, column 1: `schema` and `primary_schema` both configure the primary schema; keep `primary_schema` (line 1, column 1) and remove the legacy `schema` key
```

### `schemas`

```toml
schemas = ["auth", "billing"]
```

Names the secondary schemas the project declares beside the primary one. Each entry is a string and is trimmed; a blank entry, an entry that is not a string, and a name declared twice are each refused, and the index of the offending entry is part of the diagnostic because TOML records no position of its own for an array element:

```text
invalid configuration at line 3, column 1: duplicate schema `auth` in `schemas`
```

No entry may repeat the primary schema the file describes — the declared one, or the `public` default when neither root key names one — because a secondary list containing the primary schema would describe the same catalog twice:

```text
invalid configuration at line 3, column 1: `schemas` must not repeat the primary schema `public`; a schema is declared either as the primary one or as a secondary one, never both
```

### Resolution

The primary schema a command describes is resolved in one order:

1. the `--schema <name>` argument of that command, when it is given and its value is not blank;
2. the `primary_schema` key, or its legacy `schema` alias;
3. the `public` default.

The secondary schemas always come from `schemas`, in declaration order, whichever step selected the primary one. An explicit flag therefore moves the primary schema alone: it neither reorders nor drops the declared secondaries.

A flag naming a declared secondary schema is a collision the file itself cannot detect, because the file cannot know which primary schema a command will select. The command refuses it:

```text
the secondary schema `app` of the project configuration repeats the primary schema `app` selected for this command; a schema is declared either as the primary one or as a secondary one, never both
```

The resolved selection is what every catalog statement binds, so the snapshot the generator writes, the digest the drift gate compares, the marker a cold start records, and the hash a checkpoint anchors all describe the same schemas. Naming a schema does not prove it holds anything: a selection whose schemas hold no table or view while the database keeps relations in other schemas is reported as a warning by the command that reads the catalog, and the command continues.

## Code generation settings

The `[codegen]` table holds the three settings the generator reads.

### `domain_types`

```toml
[codegen]
domain_types = true
```

A boolean that decides whether the generator emits the domain types of the application: with it enabled a `uuid` column becomes `uuid::Uuid`, a `json` column `serde_json::Value`, a `timestamp` column `chrono::NaiveDateTime`, a `date` column `chrono::NaiveDate`, and a `numeric` column `rust_decimal::Decimal`, instead of the `String` the engine-neutral mapping produces. The generated module then names those types, so the application enables the matching features of the library.

`--domain-types` on the command line enables the same mapping for a single run. It can only enable: a file that already enables the mapping is unaffected, and nothing on the command line switches it off again, so a project states its choice once in the file.

### `type_overrides`

The `type_overrides` table maps a schema path onto the Rust type generated for that column, which is how a domain newtype survives a regeneration:

```toml
[codegen.type_overrides]
"users.id" = "uuid::Uuid"
users.created_at = "chrono::NaiveDateTime"
wallets.address = "bitcoin::Address"
"transactions.txid" = "bitcoin::Txid"
```

A path names a column of the primary schema as `table.column` and a column of a secondary schema as `schema.table.column`, so it carries two or three segments. One path may be written in three equivalent spellings:

| Spelling | Written as | Path |
|---|---|---|
| Quoted | `"auth.users.id" = "uuid::Uuid"` | `auth.users.id` |
| Dotted | `users.id = "uuid::Uuid"` | `users.id` |
| Table | `[codegen.type_overrides.auth.users]` with `id = "uuid::Uuid"` | `auth.users.id` |

All three flatten onto the same entry, and the table form keeps a table with many overrides readable:

```toml
[codegen.type_overrides.auth.users]
id = "uuid::Uuid"
created_at = "chrono::NaiveDateTime"
```

An override is what keeps a domain newtype in place. A binary column would otherwise be generated as `Vec<u8>` and a text column as `String`, so `bitcoin::Txid` and `bitcoin::Address` name the types the application really binds; the newtype itself declares its conversions once, through `impl_sql_binary_newtype!` or `#[derive(SqlType)]`, in the module that declares the type.

The rules the entries obey:

- Every segment has to be non-empty, and the path has to carry two or three of them:

```text
invalid configuration at line 5, column 1: expected a `table.column` or `schema.table.column` schema path, got `users`
```

```text
invalid configuration at line 5, column 1: `users..id` has an empty segment at position 2; every segment of a `table.column` or `schema.table.column` path must be non-empty
```

- The schema a three-segment path names has to be one the project declares, because a primary-schema column is addressed as `table.column` and a secondary-schema column as `schema.table.column` for a schema declared in `schemas`. A path naming the primary schema can never reach a column, so it is refused and the two-segment spelling is offered:

```text
invalid configuration at line 4, column 1: override path `app.users.id` names the primary schema `app`; a primary-schema column is named `table.column`, so write `users.id`
```

- A path naming a schema `schemas` does not declare is refused the same way, and the diagnostic names the declared secondaries — or reports that the project declares none:

```text
invalid configuration at line 4, column 1: override path `ath.users.id` names the schema `ath`, which `schemas` does not declare; the declared secondary schemas are `auth`
```

```text
invalid configuration at line 2, column 1: override path `auth.users.id` names the schema `auth`, but the project declares no secondary schema; a secondary-schema column is named `schema.table.column` for a schema declared in `schemas`, and a primary-schema column is named `table.column`
```

- The cross-check considers the whole document, because a dotted root key may declare an override before the `schemas` key that authorises it.

- The value is the Rust type emitted verbatim into the declaration, so it has to be a syntactically valid Rust type, and an empty one is refused:

```text
invalid configuration at line 5, column 1: the Rust type cannot be empty
```

- The nullability of the column still applies. A nullable column wraps the override in `Option<...>` unless the value already spells an optional type, and the value is parsed as a Rust type to decide, so `Option<T>`, `std::option::Option<T>`, and `core::option::Option<T>` are all recognised and never wrapped twice.

- An override wins over the domain mapping, so a column carrying both a `domain_types = true` mapping and an override is generated with the override.

- Two spellings of one path collide, even though TOML treats them as distinct keys:

```text
invalid configuration at line 5, column 1: duplicate type override `users.id`
```

- A value that is neither a string nor a table of columns is refused at its key:

```text
invalid configuration at line 5, column 1: `codegen.type_overrides.users` must be a Rust type string or a table of column overrides
```

An override survives every later regeneration, because the generator reads the file and never edits it: the type is declared once and applied each time the module is written.

### `output`

```toml
[codegen]
output = "src/db/schema.rs"
```

Names the module code generation writes and verification reads, resolved against the directory of the configuration file. The key is not an artifact of the toolchain: it addresses generated application sources, so the artifact directory never contributes to it.

`src/schema.rs` closes the cascade when the key names no file: `codegen` writes
that default, and `verify` treats the conventional module as optional and falls back
to the catalog snapshot alone when it does not exist.

## The connection target

The file holds no connection setting — no key of it names a database — so the target of a command that opens one is always taken from its arguments or from the environment:

1. `--database-url <url>`;
2. the `DATABASE_URL` environment variable.

Two commands answer a target of their own, because the database they open is not the project's:

- `stage verify` requires an explicit scratch target — `--scratch-url`, with `--database-url` as its fallback — and never reads `DATABASE_URL`, because the verification creates and drops databases as the connected role and an implicit target routinely names the server that variable points at in a pipeline.
- `stage cleanup` sweeps the server behind `--scratch-url`, `--database-url`, or `DATABASE_URL`, in that order.

A command that neither takes the flag nor finds the variable is refused instead of guessing:

```text
no database URL provided: pass `--database-url <url>` or set the DATABASE_URL environment variable
```

## Reading a configuration diagnostic

Four failures are possible before any key is accepted:

| Failure | Diagnostic |
|---|---|
| The file named by `--config` does not exist | `configuration file <path> named by `--config` does not exist; a path the flag names is required, while the conventional `rurqe.toml` is optional` |
| The path named by `--config` is not a regular file | `configuration file <path> named by `--config` is not a regular file` |
| The file cannot be read | `failed to read configuration `rurqe.toml`: <operating-system error>` |
| The document is not valid TOML | `invalid configuration at line <line>, column <column>: malformed TOML section or entry: <parser diagnostic>` |
| A declaration is refused | `invalid configuration at line <line>, column <column>: <what is wrong>` |

The position is the one of the declaration that caused the refusal, taken from the parsed document rather than from a scan of its text, so a key spelled inside a comment or inside a multi-line string is never mistaken for the declaration itself. A repeated path is reported at the second spelling, which is the entry to remove.

An absent conventional file is not a failure. Nothing is read, nothing is reported, and the defaults apply: the primary schema is `public`, the secondary list is empty, no column carries an override, and every artifact lives beneath `rurqe/`.
