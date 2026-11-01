# Project configuration

`rurqe.toml` is the optional file that states what the toolchain has to know about a project before it opens a database: the schemas its catalog digests describe, and the Rust type the generator emits for a column whose engine type maps onto a domain type of the application.

The file is read from the working directory a command runs in, and its name is fixed: no flag of the toolchain points a command at another path. A project without the file keeps the defaults — no schema is named, so `public` applies, the secondary list is empty, no override exists, and the generated module carries no domain types.

The document is read strictly, because a misspelled key would generate a module with the wrong types and a misspelled schema would describe a catalog the project never targeted. An unknown key, a value of the wrong shape, a blank name, and a repeated declaration are each refused with the line and the column of the declaration that caused it, and a file that is malformed in any way aborts the command instead of being partially applied.

## The whole surface

| Key | Shape | Effect |
|---|---|---|
| `primary_schema` | string | The schema the project's catalog digests describe. |
| `schema` | string | The legacy spelling of the same setting. |
| `schemas` | array of strings | The secondary schemas described beside the primary one. |
| `codegen.domain_types` | boolean | Whether the generator emits the application's domain types. |
| `codegen.type_overrides` | table | The Rust type generated for a named column. |

Nothing else is accepted, at the root and inside `[codegen]` alike, and the refusal lands on the declaration:

```text
invalid configuration at line 1, column 1: unknown configuration section `primary_schemas`
```

```text
invalid configuration at line 6, column 1: unknown key `record_format` in the `codegen` section
```

The wording of the first refusal covers every undeclared root key, a near-miss of a real one included. A `[codegen]` value that is not a table is refused the same way:

```text
invalid configuration at line 4, column 1: `codegen` must be a table
```

The generator's own defaults — the module doc comment and the row records — are not settings of this file: the file parameterises types, not the shape of what is rendered.

## A complete file

```toml
primary_schema = "app"
schemas = ["auth", "billing"]

[codegen]
domain_types = true

[codegen.type_overrides]
"users.id" = "uuid::Uuid"
"auth.sessions.id" = "uuid::Uuid"
```

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

The `[codegen]` table holds the two settings the generator reads.

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

The rules the entries obey:

- Every segment has to be non-empty, and the path has to carry two or three of them:

```text
invalid configuration at line 5, column 1: expected a `table.column` or `schema.table.column` schema path, got `users`
```

```text
invalid configuration at line 5, column 1: `users..id` has an empty segment at position 2; every segment of a `table.column` or `schema.table.column` path must be non-empty
```

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

Three failures are possible before any key is accepted:

| Failure | Diagnostic |
|---|---|
| The file cannot be read | `failed to read configuration `rurqe.toml`: <operating-system error>` |
| The document is not valid TOML | `invalid configuration at line <line>, column <column>: malformed TOML section or entry: <parser diagnostic>` |
| A declaration is refused | `invalid configuration at line <line>, column <column>: <what is wrong>` |

The position is the one of the declaration that caused the refusal, taken from the parsed document rather than from a scan of its text, so a key spelled inside a comment or inside a multi-line string is never mistaken for the declaration itself. A repeated path is reported at the second spelling, which is the entry to remove.

An absent file is not a failure. Nothing is read, nothing is reported, and the defaults apply: the primary schema is `public`, the secondary list is empty, and no column carries an override.
