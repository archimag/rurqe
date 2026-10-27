# The `rurqe` Toolchain

`rurqe-cli` is the developer toolchain around the library: it introspects the
live database catalog, generates the typed Rust schema module, gates CI on
schema drift, and drives both migration lifecycles. Every digest the
toolchain computes — the generated module, the `schema.lock` snapshot, the
cold-start marker, and the checkpoint hash — describes the same catalog, and
which catalog that is gets decided once, per project, in `rurqe.toml`.

## Invocation

The toolchain installs as Cargo custom subcommands, and the binaries answer
three dispatch spellings:

```bash
cargo db-codegen ...        # cargo custom subcommand
cargo rurqe db-verify ...   # the `rurqe` / `db` forwarding token is optional
rurqe db-migrate ...         # direct invocation: subcommand as first argument
```

The short aliases `migrate`, `baseline`, `codegen`, `verify`, `stage`, and
`prune` are accepted wherever the `db-` prefixed form is.

Two flags are universal: `--help` / `-h` prints the command help, and
`-V` / `-v` prints the program version. They are answered in every position —
including the position of a value — so a universal flag standing where a URL
or a file name belongs is answered instead of silently consumed.
`--version` keeps a second meaning: it names the version argument of
`db-baseline`, `db-stage init`, and `db-stage commit`, so those commands keep
it for themselves.

## Project configuration: `rurqe.toml`

Commands read an optional `rurqe.toml` from the working directory. A missing
file means *no overrides and no schema selection*; a malformed or unreadable
one is a hard error, because silently ignoring a typo would generate wrong
types or describe the wrong schema.

```toml
schema = "app"

[codegen]
domain_types = true

[codegen.type_overrides]
"users.id" = "uuid::Uuid"
users.created_at = "chrono::NaiveDateTime"
```

- The root `schema` key names the database schema every catalog digest of the
  project describes. It is read by each command that compares catalogs —
  `db-codegen`, `db-verify`, `db-stage init`, `db-stage commit`, and the
  `db-migrate` actions. An explicit `--schema <name>` argument wins over the
  key, and the libpq default `public` applies when neither names a schema.
- `[codegen].domain_types` (boolean) enables the domain types (`Uuid`,
  `Json`, `Timestamp`, `Date`) in the generated module.
- `[codegen.type_overrides]` maps a `table.column` schema path onto the Rust
  type generated for that column, so domain newtypes survive code generation
  instead of degrading to the engine-neutral fallback. One entry may be
  spelled three ways — quoted (`"users.id" = "..."`), dotted
  (`users.id = "..."`), and nested (`[codegen.type_overrides.users]` with
  `id = "..."`) — and two spellings of one path collide loudly instead of
  overwriting each other.

Unknown sections and unknown `[codegen]` keys are rejected rather than
ignored, and every rejected entry is reported at the line and column of its
own declaration. The positions come from the parsed TOML tree
(`toml_edit::ImDocument`), not from a scan of the source text, so a path
spelled inside a multi-line string literal or after a `#` never displaces a
diagnostic onto its own line.

## Which schema a digest describes

PostgreSQL keeps tables in named schemas; when nothing selects one, the
toolchain introspects `public`, the libpq default. SQLite has no schema layer
at all, so the name is ignored rather than rejected and the single catalog of
the opened file is read — one project introspects both engines without
branching on the dialect.

A catalog read from an empty schema is not *wrong*, it is *empty* — and every
digest computed from an empty catalog compares nothing against nothing and
passes vacuously. When the selected schema holds no table or view while the
database keeps user relations in other schemas, introspection therefore
warns: the notice names the schemas that do hold relations and tells the
operator to pass `--schema <name>` or set `schema = "<name>"` in
`rurqe.toml`. The notice travels with the catalog — a command that builds a
report folds it in, and the plain library entry point logs it through the
`tracing` facade.

## `cargo db-codegen`

`db-codegen` introspects the live catalog and renders two artifacts: the
typed Rust schema module (`--output`, `src/schema.rs` by default) and the
deterministic catalog snapshot (`--lock-path`, `schema.lock` by default).

The generated module contains, in order:

- `use rurqe::prelude::*;`
- `pub const SCHEMA_HASH: &str = "...";` — the deterministic digest of the
  canonical catalog, embedded so the drift gate can tie the module to a
  catalog without a database round-trip;
- one `#[derive(SqlType, Clone, Copy, PartialEq, Eq, Debug)]` enum per
  introspected enum type, carrying `#[sql(rename = "...")]` on variants
  whose SQL label is not a valid Rust identifier;
- one `table!` block per table, mapping column markers onto quoted SQL names
  and Rust types, with nullability rendered as `Option<T>`;
- one row record per table — a `pub struct` plus an `impl FromRow` decoding
  it positionally.

Schema names that are not valid Rust identifiers are sanitized rather than
rejected: a table named `user.profiles` becomes `UserProfiles`, and a column
named `2fa enabled` becomes the marker `UserProfiles_2faEnabled` over the
field `_2fa_enabled`. Two schema names that reduce to the same identifier are
reported as `CodegenError::IdentifierCollision` — the generator never emits a
module with duplicate definitions.

The generated `FromRow` never indexes the value slice directly: it decodes
through `values.get(i)` and reports `RowDecodeError::ArityMismatch` on a short
response, keeping the panic-free invariant.

Type overrides reach the markers and the records verbatim:
`"users.id" = "uuid::Uuid"` renders `UsersId => "id": uuid::Uuid` and
`pub id: uuid::Uuid`, and an override that already spells its own nullability
(`Option<chrono::NaiveDateTime>`) is never wrapped a second time.

## `cargo db-verify`

`db-verify` is the CI drift gate. It introspects the live catalog and compares
it against two committed artifacts: the `schema.lock` snapshot
(`--lock-path`, `schema.lock` by default) and, when given `--schema-path`, the
`SCHEMA_HASH` constant of the generated module.

The comparison is hash-based, so formatting differences of the lock file —
line endings, surrounding whitespace — never surface as drift between two
equal catalogs. The gate's failures are typed:

- `InvalidLock` — the snapshot does not parse;
- `MissingSchemaHash` — the generated module carries no `SCHEMA_HASH`
  constant; a schema file that cannot be tied to any catalog is rejected
  instead of passing the gate by omission;
- `HashMismatch` — the constant disagrees with the hash of the live catalog;
- `Drift` — the canonical catalogs themselves differ; the report carries both
  hashes and a detailed `SchemaDiff`.

## Migration lifecycle commands

The lifecycles themselves — traditional sequential migrations and the stage +
tail model, checkpoints, the golden equation, and pruning — are covered in
the [migrations chapter](../migrations/overview.md). The commands, briefly:

- `cargo db-migrate` applies pending migrations, reports status, and repairs
  the schema history table (`--migrations-dir`, `migrations` by default);
- `cargo db-baseline --version <v> ...` records an existing database as the
  starting point without running historical migrations;
- `cargo db-stage` drives `init`, `verify`, `commit`, and `cleanup` over the
  declarative stage and the migration tail;
- `cargo db-prune --before <version>` advances the checkpoint marker and
  prunes obsolete migration scripts, with a dry-run mode.

Two rules are deliberate and worth calling out:

- The golden equation behind `db-stage verify` creates and drops databases as
  the connected role. Its scratch target must therefore be named explicitly —
  `--scratch-url <url>`, falling back to `--database-url <url>` — and the
  `DATABASE_URL` environment variable is never consulted for it: in CI an
  implicit target routinely names the production server. No implicit
  in-memory SQLite sandbox is chosen either; silently switching engines would
  exercise a different database than the project targets.
- `db-stage cleanup`, which sweeps scratch databases abandoned by dead
  writers, is the one command that does fall back to `DATABASE_URL` — it only
  connects and drops, so naming a server implicitly is safe there.
