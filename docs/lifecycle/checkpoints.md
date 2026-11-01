# Checkpoints and pruning

The migration history of a project only grows. Every release adds a script, an
applied script is bound to its recorded digest, and a clean environment replays
the directory from the first version. A checkpoint keeps that growth from
becoming permanent: it freezes the state the history has reached, so every script
below the frozen point can be deleted while the schema the script produced stays
reproducible.

```text
State(schema/) == State(Checkpoint + migrations/)
```

The left side is the declarative stage — the directory holding the complete DDL of
the target schema. The right side is how a database written before the current
stage reaches it: the checkpoint standing in for the scripts a prune removed, and
the tail holding the scripts above it. This chapter covers the two artifacts a
checkpoint consists of, the rules that keep the boundary trustworthy, the command
that publishes it, the guard that refuses a database lagging behind it, and the
prune that shortens the history.

## The boundary

A checkpoint names one point of the history and splits the directory in two.
Every script below that point is folded into the boundary: its effect is part of
the state a clean database starts from, so the file no longer has to exist. Every
script above it stays in the tail, because a database that existed before the
boundary still has to apply it.

```text
V1 → V2 → V3 → V4 → V5 → V6 → V7        the history, oldest first
                 ▲
                 │
     checkpoint.toml · checkpoint-stage/
     folded below ─┘        └── active tail: V6, V7, and every R__ script
```

The split buys two properties a plain history cannot offer once it is long:

- a clean environment reaches the current schema by replaying the declarative
  stage alone, so the cold start costs what the schema costs rather than what the
  number of releases costs;
- a database that holds data is advanced by the tail alone, script by script,
  each one recorded in the history table as it is applied, so a shortcut taken for
  an environment that carries nothing never reaches a database that carries
  something.

Neither property rests on trust. The boundary snapshot and the tail are replayed
together on one side and the declarative stage on the other, and the resulting
catalogs are compared, so the equation is proven for a project rather than
assumed for it.

## The two artifacts

A checkpoint is not one file. It is a marker and a snapshot, always written and
always read together, and the pair is what makes the pruning safe.

### The marker file

`checkpoint.toml` is a small, strictly read document:

```toml
[checkpoint]
pruned_up_to_version = "5.0"
schema_hash = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
```

| Key | Content |
|---|---|
| `pruned_up_to_version` | The version below which the history has been folded into the boundary snapshot and may be pruned. |
| `schema_hash` | The SHA-256 digest of the catalog the boundary was cut at. A database initialised from that stage recorded the same digest as its own baseline, and the guard compares the two. |

The version uses the grammar of a migration script: unsigned numeric parts
separated by `.` or `_`, compared part by part as numbers. `5.0` and `5_0`
therefore name the same version, while `5` is a different one that sorts below
`5.0` — the same ordering that decides which script runs first, so `1.2` precedes
`1.10` and `1` precedes `1.0`.

The document is read strictly. An unknown section, a misspelled or a repeated
key, a version that does not parse, and a hash that is not a full SHA-256 digest
are each refused with the location of the offending declaration, so a marker is
never read halfway.

An absent marker means no boundary and leaves the tail unaffected; a marker that
is present but malformed stays an error, because silently ignoring it would let
the tail run against exactly the outdated targets the marker exists to reject.
The same distinction separates an unreadable marker from an absent one. A path
that does not exist, and a path below a component that is not a directory, both
read as no marker at all — nothing can live there, and the write that follows
reports the unusable path with the artifact it could not produce — while a
permission failure, a damaged file, or a directory whose own metadata cannot be
inspected keeps the boundary unknown and stays an error.

`checkpoint.toml` is the default name. `--checkpoint-path <path>` points a command
of the lifecycle at a marker kept somewhere else.

### The boundary snapshot

`checkpoint-stage/` is the declarative stage copied verbatim at the moment the
boundary was cut: the same file names, the same DDL, and the same comments, which
is where a replay re-reads the transactionality of each script from.

The directory is a sibling of the marker and is named after its stem, so
`checkpoint.toml` describes `checkpoint-stage/`, while `--checkpoint-path prod.toml`
describes `prod-stage/`. Two markers in one directory therefore never share the
snapshot a commit installs.

A checkpoint is self-contained. The marker says how far the history was folded
and what the schema hashed to; the snapshot holds the schema itself. Neither half
is derived from the other, which is why the pair can be compared against the
declarative stage at any later time.

## One anchor per history line

A single marker anchors a history line, and a repository that keeps more than one
line gives each line its own marker and its own snapshot.

Every write of the pair happens inside the exclusive section of the marker — a
lock file beside it — so no commit, verification, or prune observes a half-written
pair or a half-pruned tail: between the read of the marker and the last artifact
it touches, no other writer interleaves, and a second writer on the same marker
either waits for the section or is refused with a diagnostic naming the live
owner.

The boundary only moves forward. A commit whose version sits below the recorded
one is refused:

```text
`checkpoint.toml` already anchors the history up to version 6.0; rewinding it to 5.0 would re-open pruned migrations
```

While the history is complete, deleting the marker returns the project to the
plain sequential lifecycle, because nothing below the boundary was pruned and the
directory alone describes every change. After a prune, the same deletion leaves
the tail without a starting point: the scripts that would advance a database
below the boundary are gone, and nothing rebuilds the state they produced.

## A frozen boundary

The boundary is a fact about every database that was ever initialised from it, so
it is never rewritten in place. The reasons are structural rather than
stylistic.

- What the deleted scripts produced is not lost, because the snapshot holds that
  state. A verification replays the boundary snapshot and continues with the tail
  on the same scratch state, so the equation stays checkable after the files are
  gone.
- The digest ties the two halves together. A database whose baseline recorded a
  different digest was initialised from a stage this marker does not describe, and
  the guard refuses it instead of advancing it over a schema the boundary never
  saw.
- A prune removes files and nothing else. It does not touch the marker, does not
  recompute a baseline, and does not alter a single row of
  `__rurqe_schema_history` in any database.
- A commit replaces the pair instead of editing it. The previous marker and
  boundary are parked, the freshly assembled boundary is renamed into place, and
  the marker is written last as the commit point. A process killed before the
  marker is durable leaves the previous pair restorable, and the next commit or
  verification restores it before doing its own work; a process killed after it
  leaves the committed pair live and the previous pair as debris, which the sweep
  discards.

## Publishing the boundary

A checkpoint is a release decision, not a maintenance step. The boundary is cut at
a version every deployed database has already passed: a target that stopped below
it cannot be advanced by a pruned tail, so the marker is written where the slowest
environment already stands, never where the newest script in the directory ends.

`stage commit` publishes it:

```bash
cargo rurqe stage commit --version 5.0
```

| Option | Effect |
|---|---|
| `--version <version>` | The boundary version the commit anchors. Required. |
| `--database-url <url>` | The database whose catalog is sealed. Falls back to `DATABASE_URL`. |
| `--stage-dir <dir>` | The declarative stage. Defaults to `schema`. |
| `--lock-path <path>` | The file receiving the catalog snapshot. Defaults to `schema.lock`. |
| `--checkpoint-path <path>` | The file receiving the boundary marker. Defaults to `checkpoint.toml`. |
| `--schema <name>` | The primary schema the snapshot and its digest describe. The secondary schemas come from `schemas` in `rurqe.toml`. |

The digest describes the whole selection the commit resolved — the primary schema
plus every secondary schema the project declares — so the marker anchors the
summed catalog the project manages rather than the relations of one schema.

The command runs in two phases, and the split is what makes it safe beside another
writer.

The parity gate reads the declarative stage, introspects the live catalog over the
selected schemas, materialises the stage in a disposable scratch state — a
throwaway database on the server behind the URL for PostgreSQL, an in-memory
database for SQLite — and compares the two. A stage that drifted from the database
is refused before a single artifact is written:

```text
the declarative stage `schema` does not describe the database this commit would anchor:
  <the structural differences, rendered clause by clause>
Hint: bring `schema/` and the database back into agreement — apply the migration that closes the gap, or re-init the stage from the live catalog — then re-run `stage commit`.
```

Both remedies are named because either side may be the one that is right: a
column the database gained from a script the stage does not carry is fixed in
`schema/`, while a stage that states the intended schema a database has not
reached yet is closed by the migration. The refusal also states that no artifact
was written and no interrupted commit repaired, because the gate runs before the
critical section opens.

The critical section is bounded by local filesystem work alone. Inside the
exclusive section of the marker the commit sweeps the staged siblings an
interrupted commit abandoned, repairs a commit an earlier process left halfway,
re-reads the marker and refuses to move the boundary backwards, writes
`schema.lock`, parks the previous marker and boundary, renames the freshly
assembled boundary into place, and writes the marker last as the commit point. A
commit that fails at any step leaves the previous pair intact or restores it, and
the staged snapshot is discarded on every failure path.

A successful commit reports the three artifacts it published:

```text
Committed the stage to schema.lock, snapshotted the boundary to checkpoint-stage, and anchored `checkpoint.toml` up to version 5.0 (hash <digest>, schemas `public`)
```

A repair renames artifacts into place before the command it belongs to finishes,
so a failure that follows a repair reports the side effect instead of hiding it:
the failure channel is the only output a failing command guarantees.

## Refusing a database that stopped below it

Once the history below a boundary is deleted, a database that stopped below it
cannot be advanced: the scripts that would bring it forward are exactly the ones
the prune removed. The marker turns that into a refusal instead of a silent
partial upgrade.

`migrate up` reads `checkpoint.toml` by default and a marker kept elsewhere when
`--checkpoint-path <path>` names it:

```bash
cargo rurqe migrate up --checkpoint-path prod.toml
```

A marker that is present arms two checks, both of them before a single script
executes; an absent one leaves the tail unaffected.

- **The boundary check.** The newest version recorded in `__rurqe_schema_history`
  — an applied script or the baseline a cold start wrote — has to sit at or above
  `pruned_up_to_version`. A target below it is refused:

```text
target database is at version `3.1` but the checkpoint pruned history up to `3.2` (stage hash <digest>): advance it to the boundary or rebuild it from `schema/` before applying the migration tail
```

- **The digest check.** The compared digest is either the one the database's own
  stage baseline recorded or the digest of the catalog the database really holds —
  both name the same schema — and a value other than `schema_hash` refuses the run
  the same way:

```text
schema at version `3.2` has hash `<recorded>`, but checkpoint expects `<expected>`: the recorded stage marker or the live catalog has diverged from the checkpoint
```

A history that never recorded a version at all — one holding only repeatable
scripts — sits below every boundary by definition and is refused by the first
check, which reports no recorded version instead of a number.

The marker flag belongs to `up` alone. `status` and `repair` read the history
table without a boundary, so they reject it instead of accepting it and ignoring
it:

```text
unknown option `--checkpoint-path` for `migrate status` (`--checkpoint-path` guards `migrate up` against a target older than the pruned boundary)
```

## Pruning the folded history

Pruning shortens the tail: once the boundary covers a set of scripts, their files
are deleted from `migrations/`.

### What a prune changes, and what it never changes

A prune deletes files. It does not touch the marker, does not recompute a
baseline, and does not alter a single row of `__rurqe_schema_history`. What the
deleted scripts produced is not lost, because the boundary snapshot already holds
that state and every cold start and every verification replays it.

What each kind of database sees afterwards:

| Database state | Before the prune | After the prune |
|---|---|---|
| Empty — a CI container, a new development machine | advanced by the full tail | unchanged: a cold start replays `schema/` only |
| At or above the boundary | advanced by the tail above the boundary | unchanged: every script it still needs sits at or above the boundary |
| Below the boundary | would need a script the boundary folded in | refused with a diagnostic naming the version it stopped at |

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

### The safeguards

- The marker must exist, and it must anchor the history at or above `--before`.
  A commit that never happened names the command that has to run first:

```text
`checkpoint.toml` is missing; commit the stage with `cargo rurqe stage commit --version 5.0` before pruning the history
```

- A marker below the requested version refuses the prune, because the scripts
  above it are still needed by the active tail:

```text
`checkpoint.toml` anchors the history up to version 4.0 only; pruning below 5.0 would drop scripts the active tail still needs
```

- Only versioned scripts strictly below `--before` are deleted. Repeatable
  scripts carry no version and are never deleted, because removing one would
  silently change the schema a cold start produces. A script at the boundary
  version itself stays, and it is inert, because every route through the
  lifecycle already skips what the boundary has folded in.
- The whole prune runs inside the exclusive section of the marker, so no commit,
  verification, or second prune observes a half-pruned tail — a marker anchored to
  scripts that are already gone.
- A directory with nothing to delete reports it instead of rendering an empty run:

```text
No migration below version 5.0 to prune in migrations
```

- A directory that does not exist reports itself the same way:

```text
No migration directory `migrations` to prune
```

The result of the pair is a history that still upgrades every database that
exists, still initialises an empty one in the time the stage takes, and can still
be proven equal to the declarative stage it describes.
