# Migrations

A migration is a forward-only SQL script kept in the repository and applied to a
database. The applied entry is recorded in a history table together with the
checksum of the script, so an edit to a script that already ran is detected
instead of being silently accepted, and a failed attempt is remembered until it
is repaired.

Versioned scripts carry an ordered version and run in that order exactly once.
Repeatable scripts carry no version and run again whenever their contents
change, which is the shape a view or a function definition takes.

The same scripts are applied from two places: the toolchain runs them from the
command line, and an application binary can carry them inside itself and apply
them at startup. Both paths share one naming grammar, one checksum algorithm,
and one history table, so a database advanced by a container start ends up
exactly where a command-line run would have left it.

## The migrations directory

Scripts are read from `migrations/` by default. `--migrations-dir <dir>` points a
command at another directory, for a project that keeps its scripts somewhere
else.

* Only files ending in `.sql` are scripts. Every other file and every
  subdirectory of the directory is ignored.
* Scripts are listed in file-name order and then sorted by the run order
  described below, so the order the filesystem reports never decides what runs
  first.
* A missing directory holds no migrations, which is what a project that has not
  started a migration history looks like. A directory that exists but cannot be
  read is reported rather than taken for an empty one, and a script whose name
  follows no convention is reported rather than skipped: a silently ignored
  script would apply an incomplete history.

A directory therefore holds scripts of both kinds side by side:

* `V1__create_users.sql` — a versioned script, version `1`;
* `V2_1__add_orders.sql` — a versioned script, version `2.1`;
* `R__active_users.sql` — a repeatable script defining the view;
* `V4__reindex_orders.notx.sql` — a versioned script that runs outside a
  transaction.

## Script names

| Script name | Kind | Applied |
|---|---|---|
| `V<version>__<description>.sql` | versioned | once, in ascending version order |
| `R__<description>.sql` | repeatable | again whenever the contents change |

The rules the names obey:

* The extension is `.sql`, and the version and the description are separated by
  a double underscore.
* A version is a sequence of unsigned numeric parts separated by `.` or `_`, so
  `V2_1__add_orders.sql` and `V2.1__add_orders.sql` declare the same version.
  Ordering compares the numeric parts rather than the text: `1.2` sorts before
  `1.10`, and `1` before `1.0`.
* A description has its underscores rendered as spaces, so `add_orders` reaches
  the history table and the status report as `add orders`.
* Repeatable scripts carry no version at all. They run after every versioned
  script, ordered by script name.

A name outside the grammar is refused before any connection is opened:

```text
invalid migration script name `V1_init.sql`: expected `V<version>__<description>.sql` or `R__<description>.sql`
```

A version that does not parse is reported on its own, so a malformed version is
never mistaken for a malformed name:

```text
invalid migration version `1a`: expected dot- or underscore-separated numeric parts
```

Two scripts may not declare one version. The run stops before applying anything
and the diagnostic names both files:

```text
duplicate migration version 2.1: `V2_1__add_orders.sql` and `V2.1__orders.sql` declare the same version
```

## Checksums

Every script is digested with SHA-256 over its exact contents when it is read,
and the hexadecimal digest is recorded with the applied entry. A script that was
applied and then edited no longer matches the recorded digest, and the next run
refuses to continue:

```text
migration `V1__create_users.sql` has been modified after being applied: recorded 6b86b273ff34fce1…, actual 3f79bb7b435b9583…
```

The digest exists because an edited script describes a state that no database
which already ran it has reached. The remedy is a new migration carrying the
change, not an edit of the script that already ran. When the edit is intended and
the script is known not to have reached every target yet, `repair` rewrites the
recorded digest once the file has been reviewed.

Repeatable scripts are never reported as modified: a changed digest is exactly
the signal that makes them run again.

## The history table

Every applied script is recorded in `__rurqe_schema_history`. The first command
that touches the table creates it:

```sql
CREATE TABLE IF NOT EXISTS __rurqe_schema_history (
    installed_rank BIGINT NOT NULL PRIMARY KEY,
    version VARCHAR(50),
    description VARCHAR(200) NOT NULL,
    type VARCHAR(20) NOT NULL,
    script VARCHAR(1000) NOT NULL,
    checksum VARCHAR(64),
    installed_on VARCHAR(50) NOT NULL,
    execution_time BIGINT NOT NULL,
    success BOOLEAN NOT NULL
);
```

| Column | Content |
|---|---|
| `installed_rank` | The position of the entry in the execution sequence, one above the highest rank already recorded. |
| `version` | The version of a versioned script; empty for a repeatable script. |
| `description` | The description with underscores rendered as spaces. |
| `type` | `VERSIONED`, `REPEATABLE`, or `BASELINE`. |
| `script` | The file name the entry was recorded from. |
| `checksum` | The SHA-256 digest of the script contents; empty for a baseline entry. |
| `installed_on` | The installation timestamp. |
| `execution_time` | The measured execution time in milliseconds. |
| `success` | Whether the script completed. |

The table belongs to the migration engine, not to the application schema, so it
stays out of the catalog snapshot and out of the generated schema module. It is
read and written under the same run lock that applies the migrations, because
creating it on a database that never ran a migration is a schema write.

A recorded failure is never ignored: until it is repaired, every run stops and
names it.

```text
migration `V2__add_orders.sql` failed at rank 7; repair required before continuing
```

## Transactions

A script runs inside a transaction together with the entry that records it, so
the schema change and its history row commit together or not at all. A script
that fails rolls its transaction back, and the attempt is recorded as an
unsuccessful entry at the rank the script would have taken. The diagnostic
carries what the engine answered:

```text
failed to execute migration script `V2__add_orders.sql`: <diagnostic of the engine>
```

The database is left as it was before the script, so the script is fixed and the
failed entry is cleared with `repair` before the next run. A backend that cannot
write the entry at all keeps the original diagnostic in front of the operator
instead of replacing it: the failure of the script is what has to be acted on,
and a second error about the record would hide it.

A script that cannot run inside a transaction is applied outside one. The two
sequences differ in exactly one place — where the history entry is written:

```text
transactional script
    BEGIN -> execute the script -> record the applied entry -> COMMIT

non-transactional script
    record an in-progress entry -> COMMIT
    execute the script in autocommit
    replace the in-progress entry with the applied one -> COMMIT
```

### Scripts that cannot run inside a transaction

Some statements refuse to run inside a transaction block at all — `VACUUM`, a
`PRAGMA` setting that rewrites the database file, and a concurrent index build
are the ones a migration routinely meets. Such a script declares itself, either
through its file name or through a directive comment:

```sql
-- The index is built without blocking writes, so the script cannot run inside a
-- transaction.
-- transactional = false
CREATE INDEX CONCURRENTLY users_email_idx ON users (email);
```

The suffix `.notx.sql` — or the doubled form `__notx.sql`, which keeps the
script name inside the naming grammar — marks the same intent by file name, and
both spellings are matched case-insensitively.

The directive rules are narrow on purpose:

* One whole comment line has to be the directive: `-- transactional = false`
  declares it, while `-- set transactional = false here when the index needs it`
  stays an ordinary remark, because the leftover words remain after the key and
  the value.
* The keys `executeInTransaction`, `execute_in_transaction`, and `transactional`
  accept the value `false`. Only alphanumeric characters are kept and they are
  lowercased before the comparison, so spacing, underscores, the case of the key,
  and comment decoration (`*`, `;`, a trailing period) change nothing.
* Directives are read from comments alone. Text inside a string literal, a
  quoted identifier, or a dollar-quoted function body is data, and so is the body
  of a nested block comment; a directive written at the outer level of a block
  comment is honoured.

A non-transactional script commits on its own, so nothing would record the
attempt if the process died right after it. The run therefore commits an
in-progress entry first, executes the script outside any transaction, and
replaces that entry with the applied one afterwards. An interruption between the
two leaves the schema changed and the in-progress entry behind, which the next
run reports as a failed migration and `repair` clears — the entry is exactly what
makes the interruption visible instead of leaving the work unrecorded.

If the script ran but its applied entry could not be written, the run names the
divergence and the remedy instead of reporting success:

```text
migration `V4__reindex_orders.notx.sql` ran outside a transaction and changed the schema, but its history row could not be written: <diagnostic>; the recorded history now lags the database, so inspect the schema and run `repair` (or record a `baseline`) before the next run
```

A backend that cannot execute a script outside a transaction at all refuses it
before the script touches the database, and the in-progress entry is withdrawn
again, so a refusal demands no repair:

```text
the migration backend cannot execute `V4__reindex_orders.notx.sql` outside a transaction: keep the script transactional or run it on a backend that supports non-transactional execution
```

### Serialising concurrent runs

Both engines take a run lock for the whole run, so two runners cannot interleave
over one history: the second one waits for the lock before it reads anything.

SQLite serialises writers through a lock on the database file, and the adapter
materialises that lock as an open `BEGIN IMMEDIATE` transaction held for the
length of the run. A runner that cannot take it within 30 seconds gives up with
a diagnostic that names the wait instead of hanging:

```text
could not acquire the SQLite write lock within 30s: <diagnostic>; another migration runner may be holding it, retry once that run finishes
```

A non-transactional script needs the lock transaction to be committed, so the
lock is committed before such a script and taken again afterwards. Between those
two statements the write lock is free: another runner may hold it for as long as
its own run lasts, and a re-take that fails only leaves the remaining steps to
open their own transactions.

PostgreSQL serialises runners through a session-scoped advisory lock. The
adapter takes it with `pg_try_advisory_lock` and polls it every 250 milliseconds
for up to 30 seconds; a wait that runs out names the lock in both its decimal
and its hexadecimal identity. The lock belongs to the session that took it, so a
migration run owns a dedicated connection for its whole length — a connection
handed back into a pool would keep the lock held for as long as the pool keeps
the session. Releasing the lock is confirmed by the server: an answer that the
session held no such lock is reported instead of being taken for a release, and
a release the server never answered is retried when the connection closes.

## Connecting to the database

`--database-url <url>` names the target. Without it the `DATABASE_URL`
environment variable is read, and a run with neither is refused:

```text
no database URL provided: pass `--database-url <url>` or set the DATABASE_URL environment variable
```

* `postgres://` and `postgresql://` URLs, and libpq keyword/value strings such as
  `host=db.internal dbname=app`, address PostgreSQL.
* A `sqlite:` URL, `:memory:`, or a path ending in `.db`, `.sqlite`, or
  `.sqlite3` addresses SQLite.

Any other value is refused at the connection gate instead of being guessed to be
PostgreSQL. PostgreSQL sessions of this build are cleartext, so a connection
string demanding TLS is refused on every host, and a remote host reached without
`sslmode=disable` is refused as a silent downgrade.

`--schema <name>` selects the primary PostgreSQL schema whose catalog digest the
history describes, and the secondary schemas declared in `schemas` of
`rurqe.toml` are described beside it: `migrate up` compares the summed catalog of
the whole selection against a checkpoint boundary, so a project keeping its tables
outside `public` names its primary schema here and the rest of the set in the
configuration file. The flag wins over the `primary_schema` key of `rurqe.toml`
(and its legacy `schema` alias), which wins over `public`. `baseline` records no
catalog digest and takes no schema selection.

## Applying pending migrations

```bash
cargo db-migrate up
```

The command reads the directory, refuses an unusable history — a recorded
failure, an edited applied script, two scripts declaring one version — and
applies what is pending in run order: versioned scripts in ascending version
order, then the repeatable scripts whose contents differ from the recorded
digest. An invocation that names no action, `cargo db-migrate`, applies pending
migrations as well, and `cargo rurqe migrate up` is the same command reached
through the toolchain's own entry point.

The report states how many scripts ran and how long they took in total, and lists
every one of them:

```text
Applied 2 migration(s) in 14 ms
  - V2_1__add_orders.sql
  - R__active_users.sql
```

A run that finds nothing pending reports `Applied 0 migration(s) in 0 ms`.

| Option | Effect |
|---|---|
| `--migrations-dir <dir>` | The directory the scripts are read from. Defaults to `migrations`. |
| `--database-url <url>` | The database to advance. Falls back to `DATABASE_URL`. |
| `--schema <name>` | The primary schema whose catalog digest the checkpoint guard compares; the secondary schemas come from `schemas` in `rurqe.toml`. |
| `--checkpoint-path <path>` | The checkpoint marker that arms the outdated-target guard. Defaults to `checkpoint.toml`. |

The checkpoint marker belongs to the stage-based lifecycle: a project that pruned
migrations below a boundary records that boundary in a marker file, and the flag
is accepted by `up` alone, where it arms the guard that refuses a target older
than the boundary instead of applying a partial tail. `status` and `repair` read
the history table only, so they reject the flag instead of accepting and ignoring
it:

```text
unknown option `--checkpoint-path` for `migrate status` (`--checkpoint-path` guards `migrate up` against a target older than the pruned boundary)
```

## Inspecting the status

```bash
cargo db-migrate status
```

The command compares every script of the directory with the recorded history and
prints one row per script:

```text
SCRIPT                                   KIND         VERSION    STATUS
V1__create_users.sql                     VERSIONED    1          Applied
V2_1__add_orders.sql                     VERSIONED    2.1        Pending
R__active_users.sql                      REPEATABLE              Pending
```

| Status | Meaning |
|---|---|
| `Pending` | Present in the directory and never applied. |
| `Applied` | Applied, and the recorded digest matches the script. |
| `Modified` | Applied, and the script changed since. |
| `IgnoredBaseline` | Below the version a baseline entry recorded. |
| `Failed` | The recorded attempt did not complete. |

A database that never ran a migration has no history table yet, so the command
creates it under the run lock and reports every script as `Pending`. A directory
without scripts is reported rather than rendered as an empty table:

```text
No migrations found in migrations
```

## Repairing the history

```bash
cargo db-migrate repair
```

The command does not execute a single script. It purges every unsuccessful entry
and rewrites the recorded digest of an applied versioned script whose contents
changed, in one transaction, and reports what it touched:

```text
Removed 1 failed entr(ies), updated 1 checksum(s)
  - V1__create_users.sql
```

Repeatable scripts are left alone, because re-running them is their remedy rather
than a recorded digest to rewrite. A failed non-transactional script deserves a
look at the schema before the entry is cleared: its statements committed one by
one, so part of the work may already be in place and a blind re-run would apply
it twice.

## Baselining an existing database

```bash
cargo db-baseline --version 1.0.0
```

A database that already carries the schema, and a project that squashed a long
history into one foundation, need a starting point instead of a replay. The
command records a single entry of type `BASELINE` at the next rank, with the
script name `<< Baseline >>`, no digest, and the description taken from
`--description` (default `baseline`):

```text
Baselined database at rank 1 (production snapshot)
```

* `--version` is required and parses with the same grammar a script version uses.
  A missing value is refused with `missing required `--version` argument`, and a
  malformed one with `invalid baseline version `v1`: …`.
* The command refuses a database that already carries history, so a second
  baseline can never silently reset the starting point:

```text
cannot baseline database: history already contains migration or baseline version 1
```

* Versioned scripts at or below the recorded version are skipped by `up` and
  reported as `IgnoredBaseline` by `status`. Repeatable scripts are unaffected:
  they run, and run again whenever their contents change.
* The entry carries no digest, so it is never reported as modified.

## The embedded migrator

The command line is one caller of the migration engine; an application binary can
be another. The `embed_migrations!` macro bakes the scripts into the binary at
compile time, so a container start needs no `migrations/` directory mounted, no
reading of files, and no write access to a scripts directory:

```rust,no_run
{{#rustdoc_include ../examples/migrations.rs:embed}}
```

It takes pairs of script name and script text — the same names, the same version
grammar, and the same SHA-256 digests as the directory — and yields a fallible
collection, so a malformed embedded script is a startup error rather than a
silently missing migration. The collection is sorted once, the way the directory
loader sorts, and exposes the scripts it holds as a slice and by reference
iteration. An empty collection, and a collection assembled from already parsed
scripts, are available as well.

| Operation | Blocking method | Method on a dedicated blocking thread |
|---|---|---|
| Apply the pending scripts | `migrate` | `migrate_async` |
| Inspect the status | `status` | `status_async` |
| Repair the history | `repair` | `repair_async` |
| Record a baseline | `baseline` | `baseline_async` |

Every blocking method drives the backend on the calling thread and returns the
report the command line prints. The asynchronous counterparts are available with
the crate's `tokio` feature: the backend is moved into a dedicated blocking
thread and handed back together with the report, so a connection that cannot be
shared across threads stays usable after the run, and a panic inside the task
surfaces as a backend error instead of unwinding the caller.

The blocking methods stay callable from an asynchronous runtime by default: a
call that blocks where blocking is illegal is reported as a warning and
proceeds. Compiling the crate with the `strict-async` feature turns that warning
into a refusal at the blocking entry points, because a worker thread and a
blocking-pool thread cannot be told apart from inside the thread; the `*_async`
wrappers run the same work on the blocking thread and are unaffected by the
feature. A startup path that has to apply migrations inside an asynchronous
runtime therefore awaits the asynchronous counterpart, or runs the blocking one
on a plain thread.

Because the collection is baked in, the binary and the scripts it carries cannot
drift apart: the digests are computed from the same text the binary runs, and the
entries a container writes match the entries the command line would have written
for the same files.
