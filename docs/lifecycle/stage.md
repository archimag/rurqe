# Stage-based schema evolution

The stage lifecycle keeps the schema of a database in two descriptions that have to converge: a declarative stage holding the complete DDL of the target schema, and a short tail of migration scripts applied on top of a recorded boundary. A clean environment is initialised from the stage, an existing database is advanced through the tail, and a checkpoint marker records how far the history has been folded into the stage and may be pruned.

Both descriptions end on one schema:

```text
State(schema/) == State(Checkpoint + migrations/)
```

The left side states where the database has to end up. The right side states how a database written before the current stage reaches the same place. A migration pruned below the checkpoint is not lost by the pruning: it is folded into the boundary snapshot every later cold start begins from.

This chapter covers the declarative stage, the cold start, the commit that seals a boundary, the checkpoint marker, the verification of the two sides, and the pruning of the history.

## Project layout

```text
project/
├── schema/                    # declarative stage (default `--stage-dir`)
│   ├── billing.sql
│   └── users.sql
├── migrations/                # active tail (default `--migrations-dir`)
│   ├── V6__add_credits.sql
│   └── R__active_users.sql
├── checkpoint.toml            # boundary marker (default `--checkpoint-path`)
├── checkpoint-stage/          # boundary snapshot, named after the marker
├── schema.lock                # catalog snapshot of the committed stage
└── rurqe.toml                 # primary_schema, schemas, and codegen settings
```

- `schema/` holds the complete DDL of the target schema, not a delta. It is the single description a developer or a model reads to learn what the schema is.
- `migrations/` holds the active tail: the versioned and repeatable scripts that advance a database which already exists and holds data.
- `checkpoint.toml` records the boundary below which the history has been folded into the stage. It is written by `stage commit` and is optional: a project without a marker keeps the sequential lifecycle alone.
- `checkpoint-stage/` is the verbatim snapshot of the declarative stage the boundary was cut at. It is what a verification replays before the tail.
- `schema.lock` is the canonical catalog snapshot of the committed stage; its digest is the hash the marker anchors.

## The working cycle

```text
schema/  ──▶ migrations/  ──▶ migrate up    (an existing database advances)
   │                            stage init    (a clean database starts at the stage)
   │
   └──▶ stage commit ──▶ checkpoint.toml, checkpoint-stage/, schema.lock
                              │
                              └──▶ prune --before <version> ──▶ scripts below the boundary deleted
```

1. The declarative stage changes: a table is added, a column is altered, an index is dropped.
2. A migration in `migrations/` carries the same change as a delta for the databases that already exist.
3. `migrate up` advances an existing database through the tail, while `stage init` initialises a clean one from the stage and records the stage version as its starting point.
4. Once every deployed database has passed the release, `stage commit` seals the current stage as the new boundary, and `prune --before <version>` deletes the scripts the boundary now covers.

## The declarative stage

The stage is a directory of `*.sql` files — `schema/` by default — holding the complete DDL of the target schema. Everything the schema consists of belongs there: tables with their columns, constraints, and defaults, the indexes and views over them, the triggers, and the enum types the columns use.

```sql
CREATE TABLE users (
    id BIGINT PRIMARY KEY,
    email VARCHAR(200) NOT NULL
);

CREATE INDEX users_email_idx ON users (email);
```

The rules the directory obeys:

- Only files ending in `.sql` are scripts. Every other file and every subdirectory of the directory is ignored.
- The scripts are applied in file-name order, so the order the file system reports never decides what runs first.
- A directory that does not exist and a directory holding no script both refuse the command that needed it, and a script that cannot be read aborts the run: a silently skipped script would initialise the database with an incomplete schema.
- A script runs inside a transaction unless it declares otherwise. A non-transactional script is marked by the `.notx.sql` (or `__notx.sql`) suffix of its file name, or by one whole comment line:

```sql
-- transactional = false
CREATE INDEX CONCURRENTLY users_email_idx ON users (email);
```

The directive accepts the keys `executeInTransaction`, `execute_in_transaction`, and `transactional`, and the value `false`. Only alphanumeric characters are kept and lowercased before the comparison, so spacing, underscores, the case of the key, and comment decoration change nothing; a line carrying leftover words after the key and the value stays an ordinary remark. Directives are read from comments alone: text inside a string literal, a quoted identifier, a dollar-quoted body, or a nested block comment is data.

The stage carries a digest of its own: SHA-256 over every script name and body in application order. Reordering the directory does not change the digest, while editing any script does.

The directory is the default. `--stage-dir <dir>` points a command at a stage kept somewhere else, and every command of the lifecycle accepts it.

## Cold start

A clean database — a CI container, a fresh development machine, an empty fixture — is initialised from the declarative stage in one command:

```bash
cargo rurqe stage init --version 5.0
```

| Option | Effect |
|---|---|
| `--version <version>` | The version the stage initialisation is recorded at. Required. |
| `--stage-dir <dir>` | The directory holding the declarative DDL. Defaults to `schema`. |
| `--description <desc>` | The description recorded in the history row. Defaults to `stage`. |
| `--database-url <url>` | The database to initialise. Falls back to `DATABASE_URL`. |
| `--schema <name>` | The primary schema the recorded catalog digest describes. The secondary schemas come from `schemas` in `rurqe.toml`. |

The run applies the scripts in file-name order. Every transactional script shares one transaction, so a failure among them leaves the database exactly as it was, while a script declaring itself non-transactional runs outside that transaction, statement by statement, because the engine would refuse it inside a block.

The run ends by recording one baseline entry in `__rurqe_schema_history`. The entry carries the stage version, the description, and the digest of the catalog the stage produced — the digest `schema.lock` records and the generated `SCHEMA_HASH` embeds, because it is derived from the catalog through the engine that really executed the DDL. A backend that cannot introspect itself anchors the digest of the stage directory instead.

The digest describes the whole selection the command resolved — the primary schema plus every secondary schema the project declares — so the marker anchors the summed catalog the project manages rather than the relations of one schema.
Because the entry is a baseline, every versioned migration at or below the version counts as applied, and the tail above it runs on the next `migrate up`. Repeatable scripts are unaffected and run whenever their contents change.

```text
Initialised the database from 2 script(s) at stage 5.0 in 41 ms
  - billing.sql
  - users.sql
```

A database that already records the same stage at the same version with the same digest is left untouched:

```text
Database already initialised at stage 5.0; nothing to apply
```

Every other state is refused instead of being initialised over. A stage marker carrying a different digest names both descriptions that diverged:

```text
the database already carries a stage marker for version `5.0` with a diverged digest; re-initialise it from scratch or commit the current stage first
```

A database that records any other history at all — an applied migration, a baseline recorded by hand — is not a clean database, and the cold start refuses it.

A stage holding a non-transactional script cannot be applied atomically, so the run commits an in-progress entry before the first of those scripts and replaces it with the applied entry once every script has run. An attempt interrupted in between stays visible in the history, and the next run reports it instead of treating the database as an unknown schema:

```text
an earlier cold start was interrupted while applying stage `5.0`: the database carries the in-progress marker recorded before its non-transactional scripts ran, so it may hold a partially initialised schema; `repair` clears the marker, and the database itself is re-initialised from scratch before the cold start runs again
```

The two refusals of the directory itself are reported before any connection is opened:

```text
declarative stage `schema` does not exist; it holds the `*.sql` DDL of the target schema
```

```text
declarative stage `schema` holds no `*.sql` script to apply
```

## The active tail

A database that holds data is never re-initialised; it is advanced. `migrate up` reads `migrations/`, refuses an unusable history — a recorded failure, an edited applied script, two scripts declaring one version — and applies what is pending: versioned scripts in ascending version order, then the repeatable scripts whose contents differ from the recorded digest. A database initialised from a stage at version `5.0` skips every versioned script at or below `5.0`, because the stage already describes that state.

`migrate up` takes one further flag that belongs to the stage lifecycle:

```bash
cargo db-migrate up --checkpoint-path checkpoint.toml
```

The marker arms the fail-safe protection of a pruned history. A target whose newest recorded version sits below `pruned_up_to_version` cannot be advanced by the remaining tail, so the run is refused before a single script executes with a diagnostic naming the version the database stopped at and the boundary it fell below, and pointing at `schema/` as the way back: the database is re-initialised from the stage, or brought forward by hand, never migrated by a partial script sequence. A database whose recorded stage marker carries a digest different from the marker's `schema_hash` is refused the same way, because the database was initialised from a stage the checkpoint does not describe.

The flag is accepted by `up` alone. `status` and `repair` read the history table without a boundary, so they reject the flag instead of accepting it and ignoring it.

## Committing a stage

When the stage has moved forward and every deployed database has passed the release that brought it there, the current stage is sealed as the new boundary:

```bash
cargo rurqe stage commit --version 5.0
```

| Option | Effect |
|---|---|
| `--version <version>` | The boundary version the commit anchors. Required. |
| `--database-url <url>` | The database whose catalog is sealed. Falls back to `DATABASE_URL`. |
| `--stage-dir <dir>` | The directory holding the declarative DDL. Defaults to `schema`. |
| `--lock-path <path>` | The file receiving the catalog snapshot. Defaults to `schema.lock`. |
| `--checkpoint-path <path>` | The file receiving the boundary marker. Defaults to `checkpoint.toml`. |
| `--schema <name>` | The primary schema the snapshot and its digest describe. The secondary schemas come from `schemas` in `rurqe.toml`. |

The command runs in two phases, and the split is what makes it safe to run beside another writer.

The preparation reads the declarative stage, introspects the live catalog over the selected schemas, compares the two, renders the snapshot and the marker, and assembles the boundary snapshot in a hidden sibling of the directory it replaces. The comparison is a gate: the snapshot and the marker describe the live database while the boundary snapshot replays the stage, so committing them while they disagree would seal a triple that contradicts itself. A stage that drifted from the database fails the commit before any artifact is written, with the structural differences rendered in the wording of a commit and both remedies named — apply the migration that closes the gap, or re-initialise the stage from the live catalog. The stage is materialised for that comparison in a disposable scratch state, never over the live database: a throwaway database on the server behind the URL for PostgreSQL, an in-memory database for SQLite.

The critical section is bounded by local filesystem work alone. It opens the exclusive section of the checkpoint marker — the `checkpoint.lock` file beside it — and, inside it:

1. sweeps the staged siblings an interrupted commit abandoned next to the artifacts;
2. repairs a commit an earlier process left halfway, restoring the parked pair the marker describes;
3. re-reads the marker and refuses to move the boundary backwards;
4. writes `schema.lock`;
5. parks the previous marker and boundary, renames the freshly assembled boundary into place, and writes the marker last as the commit point;
6. discards the parked pair and flushes the directory holding it.

A commit that fails at any step leaves the previous pair intact or restores it, and the staged snapshot is discarded on every failure path. A second writer on the same marker either waits for the section or is refused with a diagnostic naming the live owner.

The rewind guard states the consequence of moving the boundary backwards:

```text
`checkpoint.toml` already anchors the history up to version 6.0; rewinding it to 5.0 would re-open pruned migrations
```

A successful commit reports the three artifacts it published:

```text
Committed the stage to schema.lock, snapshotted the boundary to checkpoint-stage, and anchored `checkpoint.toml` up to version 5.0 (hash 4f1d9c2e..., schemas `public`)
```

### What a crash leaves behind

The marker is the commit point, because a verification only observes the pair once that file is durable. A process killed before it leaves the previous marker and boundary parked under hidden names, and the next `stage commit` or `stage verify` restores them before doing its own work. A process killed after it leaves the committed pair live and the previous pair as debris, which the sweep discards.

A repair renames artifacts into place before the command it belongs to finishes, so a failure after the repair states the side effect instead of hiding it:

```text
note: this command repaired an interrupted commit before it failed, so the workspace already changed underneath it; the repair reported:
```

## The checkpoint marker

```toml
[checkpoint]
pruned_up_to_version = "5.0"
schema_hash = "4f1d9c2e..."
```

| Key | Content |
|---|---|
| `pruned_up_to_version` | The version below which the history has been folded into the boundary snapshot and may be pruned. It uses the version grammar of a script, so `5.0`, `5_0`, and `5` declare the same version. |
| `schema_hash` | The SHA-256 digest of the catalog snapshot the boundary was cut at — the digest `schema.lock` records. |

The document is read strictly. An unknown section, a misspelled or repeated key, a version that does not parse, and a hash that is not a full SHA-256 digest are all refused with the location of the offending declaration, so a marker is never read halfway. An absent marker means "no boundary" and leaves the tail unaffected; a marker that is present but malformed stays an error, because silently ignoring it would let the tail run against a database that can no longer be advanced.

The boundary snapshot is a sibling of the marker named after its stem: `checkpoint.toml` describes `checkpoint-stage/`, while `--checkpoint-path prod.toml` describes `prod-stage/`. Two markers in one directory therefore never share the snapshot a commit installs. The snapshot is written verbatim — the scripts keep their names and their comments, which is where the verification re-reads their transactionality from.

## Verifying the golden equation

The two descriptions are proven equal by materialising both of them and comparing the resulting catalogs:

```bash
cargo rurqe stage verify --scratch-url :memory:
```

| Option | Effect |
|---|---|
| `--stage-dir <dir>` | The declarative stage to replay. Defaults to `schema`. |
| `--base-stage-dir <dir>` | Overrides the boundary snapshot the marker records. |
| `--migrations-dir <dir>` | The active tail to replay. Defaults to `migrations`. |
| `--checkpoint-path <path>` | The boundary marker. Defaults to `checkpoint.toml`. |
| `--scratch-url <url>` | The disposable target both sides are built in. `--database-url` is the fallback. |
| `--schema <name>` | The primary schema both sides are materialised into and introspected over. The secondary schemas come from `schemas` in `rurqe.toml`. |

The verification first repairs an interrupted commit inside the exclusive section of the marker and reads the marker, the tail, and the boundary snapshot there. An explicit `--base-stage-dir` wins; without it, the snapshot `stage commit` recorded next to the marker is loaded.

The two sides are then built:

- the anchored side replays the boundary snapshot, verifies its catalog against `schema_hash` before a single tail script runs, and continues with the tail on the same sandbox, skipping the scripts at or below `pruned_up_to_version` because the boundary snapshot already folds them in;
- the declarative side replays `schema/` and nothing else.

Both catalogs are introspected over the selected schemas and compared structurally: tables, columns, types, nullability, defaults, primary and foreign keys, check constraints, indexes, enums, views, and triggers. A difference is reported as a divergence naming every finding, and the command fails; equivalence is reported as one line:

```text
Golden equation holds: State(schema) == State(Checkpoint + migrations) (hash 4f1d9c2e..., schemas `public`, scratch `:memory:`)
```

A checkpoint whose boundary describes a non-empty schema requires a boundary snapshot. Without one the tail would be replayed over an empty database and a consistent project would be reported as diverged, so the verification refuses the equation and names the boundary version instead. An empty boundary is the exception: an empty scratch database reproduces it on its own.

### The scratch database

The scratch is named explicitly. `--scratch-url` wins, `--database-url` is its fallback, and `DATABASE_URL` is deliberately never consulted: the verification creates and drops databases as the connected role, so an implicit target routinely names the server that variable points at in CI. No implicit sandbox is chosen either, because replaying schema DDL into a target different from the one the project addresses would hide the substitution.

```text
no scratch database URL provided: pass `--scratch-url <url>` (or `--database-url <url>`); the golden equation needs a disposable target such as `:memory:`, an empty SQLite file, or a PostgreSQL URL, and never reads DATABASE_URL
```

- `:memory:` runs both sides in memory, and a SQLite file is used only while it is empty and is deleted when its side ends, so no object, setting, or database-header field reaches the next side.
- A PostgreSQL URL creates a throwaway `rurqe_scratch_…` database on the server behind it and drops it once the verification ends, so the connected role needs the `CREATEDB` privilege for the duration. Neither side runs inside a transaction block of its own, so statements such as `CREATE INDEX CONCURRENTLY` stay replayable.
- A selection whose schemas hold no relation while the scratch keeps user relations elsewhere contributes the vacuous-selection warning to the report, so an equation over an empty catalog is never certified silently.

## Pruning the history

Once the boundary covers a set of scripts, they are deleted:

```bash
cargo rurqe prune --before 5.0
```

| Option | Effect |
|---|---|
| `--before <version>` | The version the history is pruned below. Required. |
| `--migrations-dir <dir>` | The directory the scripts are read from. Defaults to `migrations`. |
| `--checkpoint-path <path>` | The boundary marker. Defaults to `checkpoint.toml`. |
| `--dry-run` | List what would be removed without removing it. |

| Output | Meaning |
|---|---|
| `pruned migrations/V4__add_index.sql` | The script was removed. |
| `would prune migrations/V4__add_index.sql` | The script matches the selection and `--dry-run` was given. |
| `Pruned 2 migration(s) below version 5.0` | The summary of a real run. |
| `Would prune 2 migration(s) below version 5.0` | The summary of a dry run. |

The rules the command obeys:

- The marker must exist, and it must anchor the history at or above `--before`. A commit that never happened names the command that has to run first:

```text
`checkpoint.toml` is missing; commit the stage with `cargo rurqe stage commit --version 5.0` before pruning the history
```

- A marker below the requested version refuses the prune, because the scripts above it are still needed by the active tail:

```text
`checkpoint.toml` anchors the history up to version 4.0 only; pruning below 5.0 would drop scripts the active tail still needs
```

- Only versioned scripts strictly below `--before` are deleted. Repeatable scripts carry no version and are never deleted by the prune, because removing one would silently change the schema a cold start produces. A script at the boundary version itself stays.
- The marker is never rewritten and no baseline is recomputed: the boundary stays exactly what the commit sealed, which is why pruning needs no squash.
- The whole prune runs inside the exclusive section of the marker, so no commit, verification, or second prune observes a half-pruned tail: an anchored marker whose scripts are already gone.

A directory with nothing to delete reports it instead of rendering an empty run:

```text
No migration below version 5.0 to prune in migrations
```

## Sweeping abandoned scratch databases

A verification drops the PostgreSQL scratch database it created, but a process killed mid-verification cannot, so the server accumulates `rurqe_scratch_…` databases nobody owns any more. The sweep removes them:

```bash
cargo rurqe stage cleanup --scratch-url postgres://app@db.internal/app --older-than 86400 --yes
```

| Option | Effect |
|---|---|
| `--scratch-url <url>`, `--database-url <url>` | The server whose catalog is swept. Falls back to `DATABASE_URL`. |
| `--older-than <secs>` | The age above which a scratch database counts as abandoned. |
| `--yes`, `-y`, `--force` | Confirm the drop. |

The catalog is read exactly once, so the list the confirmation names is the very list `--yes` drops. A database becomes a candidate only when all three conditions hold: the connecting host tagged it as its own, its recorded creation second is older than the threshold, and the process recorded in its name may no longer be alive on this host. Without confirmation the command refuses and names the candidates it found:

```text
`stage cleanup` requires `--yes` (or `--force`) to confirm dropping stale scratch databases; found 2 stale database(s) on `postgres://app@db.internal/app` older than 86400 second(s):
  - rurqe_scratch_...
  - rurqe_scratch_...
```

A SQLite scratch has nothing to sweep, because every verification opens it fresh in memory or in the file it is pointed at:

```text
Nothing to sweep: `:memory:` addresses a SQLite scratch database, which a verification opens in memory or in the file it is pointed at
```

Every host sharing one PostgreSQL server is given its own `RURQE_HOST_ID`. The process guard keeps a database whose recorded process is live on this host, while a scratch database created under a duplicated host id by another machine is swept by age alone; one that is genuinely abandoned is removed manually with `DROP DATABASE`.
