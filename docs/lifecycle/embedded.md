# Running migrations in production

A migration reaches a database from one of two places. The command line reads the
scripts from `rurqe/migrations/` and applies them as a process of its own. An
application binary carries the same scripts inside itself and applies them while
it starts.
time, and the scripts a binary runs are exactly the scripts it was built from.
This chapter calls that arrangement embedded migrations.

The scripts and the record do not change between the two routes. An embedded
migration carries the same file name, the same version grammar, and the same
SHA-256 digest as the directory holds, and it is recorded in the same history
table with the same kind and the same rank. A database advanced by a container
start ends exactly where a command-line run over the same files would have left
it.

What changes is where the run happens. The command line is the working loop of a
developer who edits a script and applies it; the embedded run is the production
stage of the lifecycle, and all of it happens before the service answers its
first request.

## Embedding the scripts

`embed_migrations!` bakes the migration scripts of one directory into the binary
and yields a fallible collection:

```rust,no_run
{{#rustdoc_include ../examples/migrations.rs:embed}}
```

The directory it scans is decided in one of two ways.

- A call without an argument starts at the manifest directory of the crate being
  compiled, climbs its ancestors for a `rurqe.toml`, and resolves the migration
  directory through that configuration: its `[migrations].dir` key when the file
  declares one, `<root>/migrations` otherwise. Without a configuration on that
  walk the conventional `<manifest>/rurqe/migrations` applies.
- One optional argument replaces that walk with a path resolved against the
  manifest directory. The filesystem decides what the path means: a directory is
  the migration directory itself, while a file is a `rurqe.toml` whose keys
  resolve the directory against the directory holding that file.

Only the `*.sql` files of that directory are baked in. The `checkpoint-stage/`
snapshot and the `checkpoint.toml` marker sit beside the scripts and stay
artifacts of the toolchain, so neither reaches the collection. Every script is
included from its absolute path, which registers it as an input of the
compilation: editing a script recompiles the binary that carries it.

A directory holding no `*.sql` script, a path naming neither a directory nor a
configuration file, and a configuration the strict parser rejects each stop the
compilation with an error naming the path and the remedy.

The collection parses the script names and digests the SQL text when it is built,
so a script whose name follows no convention, a version that does not parse, and
two entries sharing one script name stop the process at startup instead of
leaving a gap in the recorded history. The collection is sorted at construction,
the way the directory is sorted: versioned scripts in ascending version order,
followed by the repeatable scripts ordered by script name.

## The embedded collection

`EmbeddedMigrations` holds the parsed scripts and exposes them without hiding what
they are:

- `len` and `is_empty` state how many scripts the binary carries;
- `migrations` returns them as a slice, and the collection iterates by reference
  and by value;
- `to_vec` copies them out;
- `empty` builds a collection holding no script;
- `from_migrations` wraps already parsed migrations, sorts them
  deterministically, and refuses two entries sharing one script name.

A script name identifies one migration. Two entries under one name would apply
two different bodies under a single recorded identity: the first one is recorded,
the second is re-applied as a modified script and recorded again, and the history
ends up with two rows for one name. Both constructors refuse such a set, and a run
refuses a list assembled by hand before it applies anything.

## Starting the service

Every lifecycle operation exists twice: a blocking method that drives the backend
on the calling thread, and a counterpart that runs the same work on a dedicated
blocking thread. The counterparts are available with the `tokio` feature.

| Operation | Blocking method | Method on a dedicated blocking thread |
|---|---|---|
| Apply the pending scripts | `migrate` | `migrate_async` |
| Inspect the status | `status` | `status_async` |
| Repair the history | `repair` | `repair_async` |
| Record a baseline | `baseline` | `baseline_async` |

`migrate` applies what is pending — the versioned scripts in ascending version
order, then the repeatable scripts whose contents differ from the recorded digest
— and returns how many scripts ran, which ones, and how long the run took. A run
that finds nothing pending applies no script. A database that never ran a
migration holds no history table yet, and the run creates it under the same run
lock the command line takes.

`status` executes no script and returns one entry per script, carrying the script
name, its kind, its version, and its status: `Pending` for a script never applied,
`Applied` for one whose recorded digest matches, `Modified` for one that changed
after it ran, `IgnoredBaseline` for one below the version a baseline recorded, and
`Failed` for an attempt that did not complete. A status read on a database that
was never initialised creates the history table first, under the same run lock,
because creating it is a schema write.

`repair` executes no script either. It purges every unsuccessful entry and
rewrites the recorded digest of an applied versioned script whose contents
changed, and it reports what it removed and which digests it rewrote.

`baseline` records a starting point for a database that already carries the
schema. It takes the version and the description of the entry, and a database
whose history already holds an entry is refused instead of silently restarted; the
diagnostic names the version or the script that is already recorded.

## A failure before the first request

A run reports failure in a typed error of the migration crate, so a startup
distinguishes the cases instead of matching on text:

- `DuplicateScriptName` refuses a collection built with two entries under one
  script name, before a connection is opened.
- `FailedMigrationAlreadyApplied` refuses a run over a history that records an
  unsuccessful attempt: the attempt has to be cleared first.
- `ChecksumMismatch` refuses an applied versioned script whose contents no longer
  match the recorded digest.
- `BaselineAlreadyExists` refuses a baseline on a database whose history already
  holds an entry.
- `MigrationExecutionFailed` reports a script that did not complete: its
  transaction is rolled back, the unsuccessful attempt is recorded, and nothing
  that follows it has run.
- `Backend` carries a driver failure, and the same variant carries the refusal the
  `strict-async` feature raises for a blocking call on a runtime thread.

The run performs no repair of its own. A refused history and a failing script both
stop it, and the deliberate next step belongs to an operator: fix the script, then
`repair` to clear the recorded attempt, then run again.

A service that cannot advance the schema does not serve. The startup propagates
the error with `?`, the process exits with a non-zero status, and the orchestrator
restarts it or rolls the deployment back. Serving on a schema the binary never
agreed with is the one outcome this route exists to prevent.

## Blocking, asynchronous, and the runtime boundary

The blocking methods run on the calling thread until the database answers. That is
correct on the startup path of a plain thread and illegal on a worker thread of an
asynchronous runtime, where the reactor starves for the length of the run.

- Called on a worker thread of an active asynchronous runtime, a blocking method
  reports the call as a warning and proceeds. The default keeps an existing
  startup path working, and the warning is the signal that the call belongs on the
  other route.
- Compiling the migration crate with the `strict-async` feature turns the same
  check into a refusal: every blocking entry point returns a backend error naming
  the method instead of blocking the runtime.
- Wrapping a blocking method in a manual `spawn_blocking` does not help under the
  feature, because a worker thread and a thread of the blocking pool cannot be
  told apart from inside the thread, so the refusal applies there as well. Only
  the asynchronous counterparts are sanctioned, and the feature leaves them
  untouched: they run the same work on a blocking thread of the pool.
- A panic inside the task never unwinds the caller. It surfaces as a backend
  error, and the backend itself is handed back on success alone.

Runtime detection is available with the `tokio` feature. A build without it cannot
observe a runtime at all, so neither the warning nor the refusal ever triggers
there.

A service that migrates before it serves traffic awaits the asynchronous
counterpart inside its runtime. A binary that owns its own thread applies the
blocking method before the runtime starts.

## A production startup

The order of the startup is what makes the arrangement safe:

1. Build the collection and propagate the failure with `?`: a malformed script
   stops the process before a connection is opened.
2. Open the connection the application keeps and apply the pending scripts through
   `migrate` or `migrate_async`.
3. Serve traffic only once the run reports success.

A failed transactional script rolls its transaction back and leaves the database
as it was before that script; the unsuccessful attempt is recorded, and `repair`
clears the entry once the script is fixed. A script that runs outside a
transaction commits statement by statement, so part of its work may already be in
place and deserves a look at the schema before it is re-run.

`status` and `repair` belong to a deployment that reports or repairs the record
rather than advancing the schema: neither applies a script, so the service that
advances the schema still runs `migrate` before it serves.

Because the scripts are baked in, the binary and the scripts it runs cannot drift
apart, and the digests it records are computed from the text it executed. A script
edited after the binary was built reaches the database only once the binary is
rebuilt.

## The backend the run needs

Every operation takes the backend that performs the I/O. The collection opens no
connection of its own: it hands the scripts to the backend the caller supplies, so
the connection, the transaction, and the run lock are the ones the application
already controls.

A backend implements one contract, the same one the command line drives through
its own drivers, and the embedded collection never names a driver. The workspace
ships two backends for a real database — one for PostgreSQL and one for SQLite —
inside the command-line crate, which drives them to apply a directory of scripts,
and the migration crate ships an in-memory backend its own tests run against. An
application that embeds migrations against a real database therefore supplies the
backend that fits its driver: the collection carries the scripts, and the service
carries the connection.

## The command line and the embedded collection

During development the command line is the working loop: a new script is written,
the run applies what is pending, the status shows what is left, and a repair
clears a mistake. In production one binary starts, applies the scripts it carries,
and serves traffic. Both routes share the script naming grammar, the digest
algorithm, and the history table; they differ in what each one can reach.

| Concern | Command line | Embedded collection |
|---|---|---|
| Where the scripts come from | `rurqe/migrations/` on disk, read at run time | compiled into the binary, parsed at startup |
| Applying the pending scripts | `cargo rurqe migrate up` | `migrate` / `migrate_async` |
| Inspecting the status | `cargo rurqe migrate status` | `status` / `status_async` |
| Repairing the history | `cargo rurqe migrate repair` | `repair` / `repair_async` |
| Recording a baseline | `cargo rurqe baseline --version <version>` | `baseline` / `baseline_async` |
| Cold start from the declarative stage | `cargo rurqe stage init --version <version>` | not available |
| Sealing a boundary | `cargo rurqe stage commit --version <version>` | not available |
| Verifying both descriptions | `cargo rurqe stage verify` | not available |
| Pruning the history | `cargo rurqe prune --before <version>` | not available |
| Outdated-target guard | armed by `--checkpoint-path` on `migrate up` | not available |

## What the embedded collection does not do

- It carries migration scripts only. The declarative stage, the checkpoint marker,
  and the boundary snapshot are artifacts the command line reads from the
  repository, so the cold start, the boundary commit, the verification, and the
  pruning stay command-line steps, and a binary never initialises a database from
  the stage on its own.
- It does not arm the outdated-target guard. The command line refuses a target
  older than the pruned boundary before it applies anything, while the embedded
  collection consults no marker and applies the scripts it carries, so a database
  that stopped below a pruned boundary is protected by the run that precedes the
  deployment rather than by the binary.
- It does not read files. The binary is the source of the run: a script edited on
  disk changes nothing until the binary is rebuilt.
- It does not open a connection and never names a driver: the backend is the
  caller's, and the collection holds no engine of its own.
