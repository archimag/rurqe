# Stage-based schema evolution

The stage lifecycle keeps the schema of a database in two descriptions that have to agree: a declarative stage holding the complete DDL of the target schema, and a short tail of migration scripts that brings a database written before the current stage to the same place. A checkpoint marker records where the history has been folded into the stage, so the scripts below that point can be deleted without breaking a single database that still has to reach the current schema.

```text
State(schema/) == State(Checkpoint + migrations/)
```

The left side states where the database has to end up. The right side states how a database written before the current stage gets there. The chapter covers the two lifecycles, the declarative stage, the cold start, the checkpoint and the pruning it enables, the verification of the equation, and the move of an existing project onto the lifecycle.

## Two lifecycles, one schema

A schema reaches its current state along one of two routes, and a project is free to use either.

- The **sequential lifecycle** describes the schema as a directory of migration scripts applied in order. What a database consists of is the accumulated result of every script applied to it.
- The **stage lifecycle** describes the schema as a declarative directory holding the complete DDL of the target schema, beside a short tail of scripts that advances a database written before the current stage.

The two routes are not rival implementations of one idea; they differ in what each one is asked to be good at. A sequential history is a record of changes, and that record is exactly what an upgrade of a live database needs. A stage is a description of the current state, and that description is what a reader, a fresh environment, or a verification needs. Both routes end on the same schema, and both share one history table, one script naming grammar, and one checksum algorithm, so a project that outgrows the first adopts the second without rewriting a single entry that already ran.

### Why a project starts with the sequential route

A young project holds a handful of scripts, and at that size the sequential description is exact and cheap to live with:

- The whole history is read in one sitting, so nothing has to be reconstructed from memory.
- A clean database replays the handful of scripts in the time the handful takes, so a CI container started from scratch does not spend its first minutes replaying releases that predate the code under test.
- Every script runs once against a live database together with the checksum and the history entry that record it, which is the audit trail a production upgrade is expected to leave.
- Nothing has to be decided about a boundary, because no history is old enough to fold away.

### The cost that grows with the history

The cost of the same description follows the number of releases rather than the size of the schema:

- **The schema is written nowhere.** Learning what a table consists of means reading every script that ever touched it, in version order, because the statement that created a column and the statement that altered it three releases later are equally part of the answer.
- **A clean environment pays for the whole history.** Every CI container, fixture, and development machine starts at the first script and applies its way forward, so reaching the current schema costs what the number of releases costs instead of what the size of the schema costs.
- **The history only grows.** An applied script is bound to its recorded checksum, so editing it is a refusal rather than a change, while deleting it removes the only description of that change for every database which has not applied it yet; recording a baseline by hand names a starting point without proving anything about it.
- **Ordering constrains the design.** A correction of an earlier script has to be a new script, so the directory accumulates steps whose only purpose is to preserve the order the history was written in, even where no database is expected to pass through the intermediate state again.
- **The tail and the record are one thing.** There is no point at which a script stops being required by a live database and becomes a record alone, so nothing is pruned without a guess about which databases still have to apply it.

None of this is a defect. A script applied once against a live database, carrying its checksum and its history entry, is precisely the record a production upgrade needs. The cost is paid by everything that does not need the record: the clean environment, the reviewer, and any tool that has to reason about the schema as a whole.

### What the stage adds

The stage lifecycle separates the two jobs the directory carries at once. The stage states what the schema is, the tail states how a database written before the current stage reaches it, and the checkpoint records where the two meet.

- **The stage is the description.** It holds the complete DDL of the target schema rather than a delta, so it is the one document a reader consults to learn what the schema is, and no history has to be replayed in the head to answer a question about a column.
- **The tail is the road.** It holds the versioned and repeatable scripts that a database which already exists and holds data still has to apply, and it stays short because everything below the boundary has been folded into the stage — which is what lets the history be pruned without a squash.
- **The checkpoint is the seam.** It records the version below which the history has been folded, together with the digest of the catalog the stage produced at that point, so both sides of the seam can be rebuilt and compared, and a target that stopped below the recorded version is refused instead of being advanced by a partial tail.

The split is what buys the two properties the sequential route cannot offer once the history is long:

- A clean environment is initialised from the stage in one pass, so reaching the current schema costs the size of the schema rather than the length of the history. The run records the stage version as its starting point, and the next `migrate up` skips every versioned script at or below that version, because the stage already describes that state.
- A database that holds data is never re-initialised. It is advanced by the tail alone, script by script, each one recorded in the history table as it is applied, so a shortcut taken for an environment that carries nothing never reaches a database that carries something.

Neither property rests on trust. Both descriptions are materialised side by side and their catalogs compared, so `State(schema/) == State(Checkpoint + migrations/)` is proven for a project rather than assumed for it, and a divergence is caught before a release instead of by a database that can no longer be advanced.

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

## The working day

A schema change is written twice, and the two descriptions are made to agree before the change is released.

```text
schema/  ──▶ migrations/  ──▶ migrate up    (an existing database advances)
   │                            stage init    (a clean database starts at the stage)
   │
   └──▶ stage commit ──▶ checkpoint.toml, checkpoint-stage/, schema.lock
                              │
                              └──▶ prune --before <version> ──▶ scripts below the boundary deleted
```

1. The declarative stage changes: a table is added, a column is altered, an index is dropped.
2. One migration in `migrations/` carries the same change as a delta for the databases that already exist. One task, one script: the script is the unit a reviewer reads and a deployment applies, so a change split over two scripts of the same version never becomes one release.
3. `migrate up` advances an existing database through the tail, while `stage init` initialises a clean one from the stage and records the stage version as its starting point.
4. `stage verify` states whether the two descriptions still converge, and fails the change while they disagree.
5. Once every deployed database has passed the release, `stage commit` seals the current stage as the new boundary, and `prune --before <version>` deletes the scripts the boundary now covers.

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
- The scripts are applied in file-name order, and a numeric component of a name is compared by its value, so `2_users.sql` precedes `10_orders.sql` without a leading zero. The order the file system reports never decides what runs first.
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

The tail is also what keeps a change reversible in review: the stage states the destination and the script states the road, so a reviewer reads the destination once and checks that the road reaches it.

## Fail-safe protection of a pruned history

Once the history below a boundary is deleted, a database that stopped below that boundary cannot be advanced: the scripts that would bring it forward are exactly the ones the prune removed. The marker exists to turn that into a refusal instead of a silent partial upgrade.

`migrate up` therefore takes one flag that belongs to the stage lifecycle:

```bash
cargo rurqe migrate up --checkpoint-path checkpoint.toml
```

With the marker in hand, the run performs two checks before a single script executes.

- **The boundary check.** A target whose newest recorded version sits below `pruned_up_to_version` is refused, with a diagnostic naming the version the database stopped at and the boundary it fell below, and pointing at `schema/` as the way back: the database is re-initialised from the stage, or brought forward by hand, never migrated by a partial script sequence.
- **The digest check.** The marker also anchors the digest of the catalog the boundary was cut at. A database whose own stage marker records a different digest was initialised from a stage this checkpoint does not describe, so it is refused the same way rather than being advanced over a schema the boundary never saw.

The flag is accepted by `up` alone. `status` and `repair` read the history table without a boundary, so they reject the flag instead of accepting it and ignoring it.

A history that never recorded a version at all — one holding only repeatable scripts — sits below every boundary by definition and is refused by the same check.

## The checkpoint

A checkpoint is two artifacts, and the pair is what makes the pruning safe.

### The marker file

`checkpoint.toml` is a small, strictly read document:

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

The same distinction governs an unreadable marker. A path that does not exist, and a path below a component that is not a directory, both read as "no marker" — nothing can live there, and the write that follows reports the unusable path with the artifact it could not produce. A permission failure, a damaged file, or a directory whose own metadata cannot be inspected keeps the boundary unknown and stays an error, because a runner that assumes no boundary proceeds against exactly the outdated targets the marker exists to reject.

### The boundary snapshot

`checkpoint-stage/` is the declarative stage copied verbatim at the moment the boundary was cut: the same file names, the same DDL, and the same comments, which is where the replay re-reads the transactionality of each script from.

The directory is a sibling of the marker and is named after its stem, so `checkpoint.toml` describes `checkpoint-stage/`, while `--checkpoint-path prod.toml` describes `prod-stage/`. Two markers in one directory therefore never share the snapshot a commit installs.

A checkpoint is therefore self-contained: the marker says how far the history was folded and what the schema hashed to, and the snapshot holds the schema itself. Neither half is derived from the other, which is why the pair can be compared against the declarative stage at any later time.

### Why the pair is safe

- The snapshot is what the deleted scripts produced. A verification replays it before the tail, so the state the pruned scripts created is still reproducible after their files are gone.
- The marker is never rewritten by a prune and no baseline is recomputed. Pruning removes files from the tail; it changes nothing about what any database has already recorded.
- The marker is the commit point of a commit. A process killed before it is durable leaves the previous marker and boundary parked under hidden names, and the next `stage commit` or `stage verify` restores them before doing its own work; a process killed after it leaves the committed pair live and the previous pair as debris, which the sweep discards.
- Every write of the pair happens inside the exclusive section of the marker — the `checkpoint.lock` file beside it — so no commit, verification, or prune observes a half-written pair or a half-pruned tail.

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

The version is the newest one every deployed database has already applied, not the newest script in the directory. A target that stopped below the boundary cannot be advanced by a pruned tail, so the marker is cut where the slowest environment already stands.

The command runs in two phases, and the split is what makes it safe to run beside another writer.

The preparation reads the declarative stage, introspects the live catalog over the selected schemas, compares the two, renders the snapshot and the marker, and assembles the boundary snapshot in a hidden sibling of the directory it replaces. The comparison is a gate: the snapshot and the marker describe the live database while the boundary snapshot replays the stage, so committing them while they disagree would seal a triple that contradicts itself. A stage that drifted from the database fails the commit before any artifact is written, with the structural differences rendered in the wording of a commit and both remedies named — apply the migration that closes the gap, or re-initialise the stage from the live catalog. The stage is materialised for that comparison in a disposable scratch state, never over the live database: a throwaway database on the server behind the URL for PostgreSQL, an in-memory database for SQLite.

The critical section is bounded by local filesystem work alone. It opens the exclusive section of the checkpoint marker and, inside it:

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

A repair renames artifacts into place before the command it belongs to finishes, so a failure after the repair states the side effect instead of hiding it:

```text
note: this command repaired an interrupted commit before it failed, so the workspace already changed underneath it; the repair reported:
```

## Verifying the golden equation

The two descriptions are proven equal by materialising both of them and comparing the resulting catalogs. The verification is the gate a pipeline runs before a merge:

```bash
cargo rurqe stage verify --scratch-url :memory:
```

| Option | Effect |
|---|---|
| `--scratch-url <url>` | The target a verification builds both sides in. |
| `--database-url <url>` | The fallback target when `--scratch-url` is not given. |
| `--stage-dir <dir>` | The declarative stage. Defaults to `schema`. |
| `--base-stage-dir <dir>` | An explicit boundary snapshot, in place of the one recorded beside the marker. |
| `--migrations-dir <dir>` | The tail. Defaults to `migrations`. |
| `--checkpoint-path <path>` | The boundary marker. Defaults to `checkpoint.toml`. |
| `--schema <name>` | The primary schema compared. The secondary schemas come from `schemas` in `rurqe.toml`. |

The scratch target is always named explicitly, either with `--scratch-url` or with `--database-url` as its fallback; `DATABASE_URL` is deliberately never consulted, because the verification creates and drops databases as the connected role and an implicit target routinely names the server that variable points at.

The verification first repairs an interrupted commit inside the exclusive section of the marker and reads the marker, the tail, and the boundary snapshot there. An explicit `--base-stage-dir` wins; without it, the snapshot `stage commit` recorded next to the marker is loaded.

The two sides are then built:

- the anchored side replays the boundary snapshot, verifies its catalog against `schema_hash` before a single tail script runs, and continues with the tail on the same sandbox, skipping the scripts at or below `pruned_up_to_version` because the boundary snapshot already folds them in;
- the declarative side replays `schema/` and nothing else.

Both catalogs are introspected over the selected schemas and compared structurally: tables, columns, types, nullability, defaults, primary and foreign keys, check constraints, indexes, enums, views, and triggers. A difference is reported as a divergence naming every finding, and the command fails; equivalence is reported as one line:

```text
Golden equation holds: State(schema) == State(Checkpoint + migrations) (hash 4f1d9c2e..., schemas `public`, scratch `:memory:`)
```

A checkpoint whose boundary describes a non-empty schema requires a boundary snapshot, because replaying the tail over an empty database would report a consistent project as diverged. An empty boundary is the exception: an empty scratch database reproduces it on its own.

## Pruning the history

Pruning is the step that keeps the tail short: once the boundary covers a set of scripts, their files are deleted from `migrations/`.

### What a prune changes, and what it never changes

A prune deletes files. It does not touch the marker, does not recompute a baseline, and does not alter a single row of `__rurqe_schema_history` in any database. What the deleted scripts produced is not lost, because the boundary snapshot already holds that state and every verification and cold start replays it.

What each kind of database sees afterwards:

| Database state | Before the prune | After the prune |
|---|---|---|
| Empty — a CI container, a new development machine | advanced by the full tail | unchanged: `stage init` replays `schema/` only |
| At or above the boundary | advanced by the tail above the boundary | unchanged: every script it still needs sits at or above the boundary |
| Below the boundary | would need a script the boundary has folded in | refused with a diagnostic naming the version it stopped at |

### Running the prune

A dry run lists what the boundary covers:

```bash
cargo rurqe prune --before 5.0 --dry-run
```

The run without the flag removes it:

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

### The rules the command obeys

- The marker must exist, and it must anchor the history at or above `--before`. A commit that never happened names the command that has to run first:

```text
`checkpoint.toml` is missing; commit the stage with `cargo rurqe stage commit --version 5.0` before pruning the history
```

- A marker below the requested version refuses the prune, because the scripts above it are still needed by the active tail:

```text
`checkpoint.toml` anchors the history up to version 4.0 only; pruning below 5.0 would drop scripts the active tail still needs
```

- Only versioned scripts strictly below `--before` are deleted. Repeatable scripts carry no version and are never deleted by the prune, because removing one would silently change the schema a cold start produces. A script at the boundary version itself stays, and it is inert: every runner and the verification skip what the boundary has folded in.
- The whole prune runs inside the exclusive section of the marker, so no commit, verification, or second prune observes a half-pruned tail: an anchored marker whose scripts are already gone.
- A directory with nothing to delete reports it instead of rendering an empty run:

```text
No migration below version 5.0 to prune in migrations
```

## Moving an existing project onto the stage

A project that already runs the sequential lifecycle adopts the stage lifecycle without rewriting its history. The migrations directory keeps its scripts, every database keeps its recorded entries, and the tail stays where it is: the stage is added beside it, and the boundary is sealed once.

The steps are ordered, because each one is the check on the step before it. The boundary is sealed only when the stage describes the live database, and the history is pruned only when the boundary and the tail still reproduce the stage.

```text
schema/ written ──▶ stage commit ──▶ stage verify ──▶ prune ──▶ stage init
                         │                 │               │
                         │                 │               └─ scripts below the boundary deleted
                         │                 └─ State(schema/) == State(Checkpoint + migrations/)
                         └─ schema.lock, checkpoint-stage/, checkpoint.toml
```

### 1. Write the declarative stage

`schema/` has to hold the complete DDL of the schema the database holds now: the tables with their columns, constraints, and defaults, the indexes and views over them, the triggers, and the enum types the columns use. The applied history is the description it is assembled from — replaying `migrations/` in version order produces exactly the schema to state — so the stage states that result once, as the description a reader consults instead of the history.

No command writes `schema/` from a database. The stage is the description a project commits to, so the toolchain reads it and never produces it, and what checks the description is the commit below: it materialises the stage in a disposable scratch state and compares the result against the live catalog.

### 2. Seal the boundary

```bash
cargo rurqe stage commit --version 5.0
```

`--version` is the newest version every deployed database has already applied, not the newest script in the directory. A target that stopped below the boundary cannot be advanced by a pruned tail, so the marker is cut where the slowest environment already stands, and `checkpoint.toml` records that version together with the digest of the catalog the live database reports.

The commit gates on parity. A stage that does not describe the database is refused before a single artifact is written:

```text
the declarative stage `schema` does not describe the database this commit would anchor:
  ...
Hint: bring `schema/` and the database back into agreement — apply the migration that closes the gap, or re-init the stage from the live catalog — then re-run `stage commit`.
```

Both remedies are named because either side may be the one that is right: a column the database gained from a migration the stage does not carry is fixed in `schema/`, while a stage that states the intended schema a database has not reached is closed by the migration. The tail is untouched at this point: every script stays in `migrations/`, and every database still advances through it.

### 3. Verify the equation

```bash
cargo rurqe stage verify --scratch-url :memory:
```

No `--base-stage-dir` is passed: the verification loads the boundary snapshot the commit recorded next to the marker, checks its catalog against `schema_hash` before a single tail script runs, and skips the tail scripts at or below the boundary, because the snapshot already folds them in. A SQLite project verifies against `:memory:`, while a PostgreSQL project passes a disposable URL whose role holds the `CREATEDB` privilege for the duration of the run.

### 4. Prune the folded history

A dry run lists what the boundary covers, and the run without the flag removes it:

```bash
cargo rurqe prune --before 5.0 --dry-run
cargo rurqe prune --before 5.0
```

Only versioned scripts strictly below `--before` are deleted. The script at the boundary version itself stays, and it is inert. Repeatable scripts are never pruned. The marker is not rewritten, and no baseline is recomputed.

### 5. Run the two lifecycles side by side

With the boundary committed and the history pruned, both routes describe the same schema and each one serves the environments it fits.

- A clean environment is cold-started from the stage: `cargo rurqe stage init --version 5.0`. The run records the stage version as a baseline, so the tail above the boundary still applies on the next `migrate up`, and the environment never replays the pruned history.
- A database that holds data is advanced by the tail: `cargo rurqe migrate up --checkpoint-path checkpoint.toml`. The marker arms the fail-safe, so a target whose newest recorded version sits below the boundary, and a target that records no version at all, are both refused before a single script executes instead of being advanced by a partial tail. Once the history below the boundary is pruned, the cold start is the only route left for an empty database, because the scripts that would have advanced it are exactly the ones the prune removed.
- A change is written twice, and the two descriptions are made to agree: the DDL of `schema/` states the new state, and a `V...` or `R...` script in `migrations/` carries the same change as a delta for the databases that already exist. `stage verify` fails while the two disagree, which is what keeps the pair honest between commits.
- A release every deployed database has passed seals the next boundary with `stage commit` at the new version and prunes the scripts below it, so the tail stays as short as the releases no database has passed yet.

The marker is the only record of the boundary. While the history is complete, deleting `checkpoint.toml` returns the project to the plain sequential lifecycle; after a prune the same deletion leaves the tail without a starting point, because the scripts that would advance a database below the boundary are gone and no verification can rebuild the state they produced.

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
