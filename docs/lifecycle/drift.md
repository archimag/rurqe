# Schema drift verification

A schema is described several times over its lifetime, and each description can
fall behind the others. `schema.lock` is the catalog snapshot the repository
commits, the `SCHEMA_HASH` constant embedded in the generated Rust module states
the same identity in code, the live database holds the schema that actually
exists, and a project on the stage lifecycle adds the declarative stage together
with the checkpoint plus migration tail. Drift is any disagreement among them,
and the point of checking it in a pipeline is to catch that disagreement before
a merge instead of at runtime.

Two questions are answered by two gates:

- **Does the database still match the code the project ships?** The snapshot
  gate (`cargo rurqe verify`) reads the catalog of a live database and compares it
  against `rurqe/schema.lock` and the `SCHEMA_HASH` of the generated module.
- **Do the two descriptions of the schema still converge?** The golden gate
  (`cargo rurqe stage verify`) replays the declarative stage and the checkpoint
  plus migration tail into disposable scratch databases and compares the two
  catalogs they produce.

Neither gate needs a database that already carries the schema. The snapshot gate
compares against whatever database it is pointed at, and a pipeline points it at
a throwaway database it has just built from the repository; the golden gate
builds both of its sides from nothing.

## What a verification compares

`schema.lock` is the canonical rendering of the catalogs the project declares:
its enums, views, triggers, and tables with their columns, primary keys, check
constraints, foreign keys, and indexes. Every entity carries the schema it
belongs to, every list is ordered by the composite schema-and-name key, and only
the orders
that carry meaning are kept: the labels of an enum, the columns of a primary
key, of a foreign key, and of an index stay in the order they were declared.

The identity of the schema is the digest of the catalog itself, not of the text
that carries it: the catalog is encoded into a canonical byte stream and the
SHA-256 of that stream is the digest. The generator embeds it as
`pub const SCHEMA_HASH: &str = "...";`, so the snapshot and the generated module
state the same identity, and both gates compare digests before they compare
anything else.

Formatting is not part of the identity. The digest is taken over the encoded
catalog rather than over the snapshot text, and every structural comparison
parses both sides into the catalog before comparing them, so line endings and
surrounding whitespace of the committed snapshot never surface as a finding: two
equal catalogs are equal however the file that carries one of them is spelled.

A comparison is structural, and covers:

- tables — a table declared on one side only;
- columns — the neutral type, the nullability, and the default expression;
- keys — the columns of a primary key and the whole definition of a foreign key,
  including both referential actions;
- check constraints — the expression each constraint enforces, compared after its
  internal whitespace is normalised;
- indexes — uniqueness, indexed columns, access method, partial predicate, and
  computed expressions;
- enum types — the labels and their order;
- views — the query defining the view;
- triggers — the table a trigger is attached to and its definition.

## Verifying a schema in a pipeline

Both gates run against targets the pipeline creates, so a check never touches an
environment that holds data.

1. Provision a throwaway database. A PostgreSQL service or a SQLite file is
   enough; nothing in it has to exist beforehand.
2. Build it from the repository alone, with the lifecycle the project uses — the
   migration tail, or the declarative stage for a cold start.
3. Run the snapshot gate against the database the repository just produced. Any
   difference from `schema.lock` or from `SCHEMA_HASH` means the generated
   artifacts and the migrations disagree.
4. Run the golden gate. It builds its own sides, so it needs only a target it may
   create databases on, or `:memory:`.

```bash
export DATABASE_URL=postgres://app@localhost:5432/app_ci

cargo rurqe migrate up
cargo rurqe verify

cargo rurqe stage verify --scratch-url postgres://app@localhost:5432/app_ci
```

The two gates answer different failures, so both belong in the pipeline:

- the snapshot gate catches a database advanced by something the repository does
  not describe, and a generated module left behind by a schema change;
- the golden gate catches a declarative stage that no longer matches the tail,
  including a migration that was written but never reflected in `schema/`.

A project that keeps its relations outside `public` declares its primary schema
with `primary_schema` in `rurqe.toml` (or names it with `--schema`), declares the
remaining schemas in `schemas`, and the same selection reaches every digest the
commands compare:
```bash
cargo rurqe verify --schema app
cargo rurqe stage verify --scratch-url :memory: --schema app
```

When the configuration file already names the schemas, the flags are unnecessary.
A selection that describes nothing is reported rather than certified: the warning
travels in the output of a gate that passes and inside the failure of a gate that
rejects, after its findings, so a wrong `--schema` is recognizable from the gate's
own output instead of surfacing only as relations the catalog claims to miss.
A malformed `rurqe.toml` aborts either command instead of being partially applied.

## The snapshot gate

```bash
cargo rurqe verify
```

| Option | Effect |
|---|---|
| `--database-url <url>` | The database to read. Falls back to the `DATABASE_URL` environment variable. |
| `--schema <name>` | The primary schema the catalog digest describes. Wins over the `primary_schema` key of `rurqe.toml`, and the secondary schemas come from `schemas`. |
| `--config <path>` | The configuration file whose keys resolve the paths of the invocation. Defaults to the `rurqe.toml` of the working directory. |

The command introspects the live catalog over the selected schemas, reads the
snapshot, and reads the generated module. A run that finds everything in
agreement reports it as one line:

```text
Schema verification passed against schema.lock
```

A failing verification ends the process with a non-zero status, so a pipeline
stop is the default outcome and no separate check is needed around the command.
`cargo rurqe verify` is the snapshot gate; the golden gate below is `cargo rurqe
stage verify`.

### The generated module is optional, the declared module is not

The module the gate reads is decided by the same cascade `codegen` writes with, and
the declaration decides whether the digest embedded in the code is part of the gate:

- A module named by `codegen.output` must be readable. A misspelled key fails the run
  with `failed to read schema code`; it can never pass silently because the code was
  absent.
- Without the key, the conventional `src/schema.rs` is used when it exists.
- Without either and without that file, the run falls back to the snapshot alone and
  reports `schema code not found; verifying against the lock snapshot only` as a
  warning. The verification still runs, but the module is no longer tied to the
  catalog, so a pipeline that ships generated code declares the module in its
  configuration.

### Reading a drift diagnostic

Every failure says which of the two sides the command treats as the expectation:
the snapshot is the expected state and the database is the state being measured.

#### The catalog no longer matches the snapshot

The live catalog and the snapshot describe different schemas. Both digests are
printed, followed by one line per finding:

```text
Error: Schema drift detected!
Expected schema hash: 4f1d9c2e...
Live schema hash:     8a3b1f40...
  - Table `products`: column `sku` exists in database but is missing from snapshot.
  - Table `products`: index `products_sku_idx` is in snapshot but missing from database.
  - View `active_users` definition differs between database and snapshot.

Hint: Run `cargo rurqe codegen` to update code and snapshot.
```

The finding wording follows the kind of object:

- a top-level object that the snapshot declares and the database lacks reads
  `is declared in snapshot but missing from database`;
- a member of a table — a column, a check constraint, an index — reads `is in
  snapshot but missing from database`, each line qualified with the owning table;
- an object the database declares and the snapshot lacks reads `exists in
  database but is missing from snapshot`;
- an object both sides declare differently reads `definition differs between
  database and snapshot`.

The remedy depends on which side is right. When the database carries the intended
schema — a column was added by a migration that has not been generated yet — the
snapshot and the module are stale, and `cargo rurqe codegen` refreshes both. When
the snapshot is right, the database was advanced by hand or by a migration the
repository does not contain, and the missing migration is what the run has to
add.

#### The generated module carries a different digest

The module and the live catalog disagree before the structural comparison is
reported:

```text
SCHEMA_HASH mismatch in schema code: expected 8a3b1f40..., found 4f1d9c2e...
  - Table `products`: column `sku` exists in database but is missing from snapshot.

Hint: Run `cargo rurqe codegen` to update code and snapshot.
```

The expected digest is the one the live catalog produces, the found one is the
constant in the module. The structural findings are carried along, so a stale
constant still names exactly what drifted instead of hiding it behind a digest
comparison. Regenerating the module and the snapshot together is the remedy.

#### The generated module carries no digest

A module that declares no `SCHEMA_HASH` constant cannot be tied to any catalog,
so it is refused instead of passing the gate by omission:

```text
The schema code declares no `SCHEMA_HASH` constant, so it cannot be verified against the live database.

Hint: Run `cargo rurqe codegen` to regenerate the schema code and snapshot.
```

A hand-written schema module reaches this diagnostic. The generated module always
carries the constant.

#### The snapshot itself is unreadable

A snapshot that is malformed, or one written in a format version the reader does
not know, fails the run before any comparison:

```text
invalid schema lock snapshot: <diagnostic of the reader>
```

Nothing is compared against a snapshot that could not be read, so a corrupt file
never renders as a clean pass. Regenerating the snapshot is the remedy.

## The golden gate

The snapshot gate compares a database with the code. The golden gate compares the
two descriptions of the schema with each other, and both of them are produced
by the run itself:

```text
State(schema/) == State(Checkpoint + migrations/)
```

```bash
cargo rurqe stage verify --scratch-url :memory:
```

| Option | Effect |
|---|---|
| `--scratch-url <url>` | The disposable target both sides are built in. `--database-url` is the fallback. |
| `--database-url <url>` | The fallback for `--scratch-url`. |
| `--schema <name>` | The primary schema both sides are materialised into and introspected over. The secondary schemas come from `schemas` in `rurqe.toml`. |
| `--config <path>` | The configuration file whose keys resolve the paths of the invocation. Defaults to the `rurqe.toml` of the working directory. |
A verification that finds both descriptions in agreement reports one line:

```text
Golden equation holds: State(schema) == State(Checkpoint + migrations) (hash 4f1d9c2e..., schemas `public`, scratch `:memory:`)
```

The line names the stage directory, the tail directory, the digest the stage
produces, the schema selection, and the scratch target.

### The scratch database
The scratch is named explicitly. `--scratch-url` wins, `--database-url` is its
fallback, and `DATABASE_URL` is deliberately never consulted: the verification
creates and drops databases as the connected role, so an implicit target
routinely names the server that variable points at in a pipeline. No implicit
sandbox is chosen either, because replaying schema DDL into a target different
from the one the project addresses would hide the substitution.

A run with neither flag is refused:

```text
no scratch database URL provided: pass `--scratch-url <url>` (or `--database-url <url>`); the golden equation needs a disposable target such as `:memory:`, an empty SQLite file, or a PostgreSQL URL, and never reads DATABASE_URL
```

- `:memory:` runs both sides in memory, and a SQLite file is used only while it
  is empty and is deleted when its side ends, so no object, setting, or
  database-header field reaches the next side.
- A PostgreSQL URL creates a throwaway `rurqe_scratch_…` database on the server
  behind it and drops it once the verification ends, so the connected role needs
  the `CREATEDB` privilege for the duration.
- Neither side runs inside a transaction block of its own, so a statement such as
  `CREATE INDEX CONCURRENTLY` stays replayable.
- A selection whose schemas hold no relation while the scratch keeps user relations
  elsewhere contributes the vacuous-selection warning to the report, so a
  comparison over an empty catalog is never certified silently. A gate that rejects
  renders that warning inside its failure, after the findings and before the remedy,
  so the one warning that explains the misleading findings is not lost with the
  report that never materialised.

### The boundary the tail replays over
A committed checkpoint anchors a boundary version and a digest. The boundary is
reproduced from the snapshot `stage commit` records next to the marker, so the
filtered tail is replayed over the schema it actually advances:

- the snapshot the commit wrote next to the marker is loaded, so the filtered tail is
  replayed over the schema it actually advances;
- the boundary digest is verified against `schema_hash` before a single tail script
  runs, and a snapshot that drifted from its marker is rejected with `base stage
  catalog hash mismatch: expected <checkpoint hash>, got <boundary hash>`;
- a checkpoint whose boundary describes a non-empty schema requires that snapshot.
  Without one the run is refused with `checkpoint pruned up to version 5.0 requires a
  base stage to reproduce the boundary, but none was supplied; re-run `stage commit`
  to snapshot the boundary next to `checkpoint.toml``, because replaying the tail over
  an empty database would report a consistent project as diverged.

The artifacts are read inside the exclusive section the checkpoint marker guards,
so a verification that overlaps a commit waits for it and reads the committed
pair. An interrupted commit is repaired before the boundary is loaded, and the
repair is reported in the output of the command it belonged to.

### Both descriptions diverged
A mismatch prints both digests, one line per finding, and the remedy:

```text
the golden equation does not hold: State(schema/) != State(Checkpoint + migrations/)
  stage hash: 4f1d9c2e...
  tail hash:  8a3b1f40...
  - Table `users`: column `email` is in the stage but missing from the tail.
Hint: update `schema/` to match the replayed tail, or add the migration that closes the gap.
```

The wording names the two sides as the stage and the tail: an object the stage
declares and the replayed tail lacks reads `is in the stage but missing from the
tail`, an object the tail produces and the stage lacks reads `exists in the tail
but is missing from the stage`, and an object both produce differently reads
`definition differs between the tail and the stage`.

The direction of the fix follows the same reasoning as the snapshot gate. When
the tail is right, the declarative stage has fallen behind the migrations and
`schema/` is updated. When the stage is right, a migration is missing from the
tail, and adding it is what closes the gap.

## What each gate establishes

| Finding | Snapshot gate | Golden gate |
|---|---|---|
| The live catalog differs from `schema.lock` | reported | not observed |
| The module's `SCHEMA_HASH` differs from the live catalog | reported | not observed |
| The module declares no `SCHEMA_HASH` | reported | not observed |
| The declarative stage differs from the checkpoint plus tail | not observed | reported |
| The boundary snapshot does not reproduce the checkpoint digest | not observed | reported |
| A checkpoint anchors a boundary no snapshot reproduces | not observed | reported |
| A selection that describes nothing | warned | warned |

A pipeline that runs both therefore closes the whole loop: the code matches the
database, and the declarative description of the schema matches the migrations
that produced it.
