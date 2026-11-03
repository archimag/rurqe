# Running migrations in production

A migration reaches a database from one of two places. The command line reads the
scripts the project configuration declares and applies them as a process of its
own. An application binary carries the artifacts inside itself and applies them
while it starts.

Both routes share the script naming grammar, the SHA-256 digest, and the history
table, so a database advanced by a container start ends exactly where a
command-line run over the same files would have left it.

What changes is where the run happens and what each route can reach. The command
line is the working loop of a developer who edits a script and applies it, and it
owns the steps that write artifacts. The embedded run is the production stage of
the lifecycle: it bakes those artifacts at compile time, opens no file at run
time, and finishes before the service answers its first request.

## Baking the artifacts at compile time

`embed_migrations!` bakes the lifecycle artifacts of one project configuration into
the calling binary and yields a fallible collection:

```rust,no_run
{{#rustdoc_include ../examples/migrations.rs:embed}}
```

The single optional argument names a `rurqe.toml`, resolved against the manifest
directory of the crate being compiled. The filesystem decides what the path is: a
file is the configuration, while a directory, a path naming nothing, and a
configuration the strict parser rejects each stop the compilation with an error
naming the path and the remedy. Only a file declares an artifact layout, so the
macro refuses a path that declares none instead of guessing at one.

- A call without an argument climbs the ancestors of the manifest directory for a
  `rurqe.toml` and reads the artifacts that file declares.
- Without a configuration on that walk, the conventional `<manifest>/rurqe`
  artifact layout applies.

The expansion itself parses nothing. Every artifact is baked as raw text through
`include_str!` from its absolute path, which registers it as an input of the
compilation: editing an artifact recompiles the binary that carries it. The text
then reaches `EmbeddedMigrations::from_artifacts`, which parses it at startup with
the very parsers the rest of the migration crate uses, so a malformed artifact is
rejected before any connection is opened.

Four artifacts are baked, and each one is optional on its own:

| Artifact | Baked as | Enables |
|---|---|---|
| Migration tail | the top-level `*.sql` scripts of the migration directory, as `(file name, SQL)` pairs | applying pending scripts |
| Boundary marker | the raw text of `checkpoint.toml` | rejecting a target below the pruned boundary |
| Declarative stage | the `*.sql` scripts of the stage directory, as `(file name, DDL)` pairs | initialising an empty database |
| Catalog snapshot | the raw text of `schema.lock` | verifying the catalog the run produced |

Only the scripts of the migration directory itself are baked: every other file and
every subdirectory of it is ignored, so the boundary snapshot beside the marker
stays an artifact of the toolchain and never becomes a script. A configuration
baking neither a tail script nor a stage script is refused, because a collection
with nothing to apply cannot initialise a database.

The collection carries the parsed scripts and exposes them without hiding what
they are:

- `len` and `is_empty` state how many scripts the binary carries;
- `migrations` returns them as a slice, and the collection iterates by reference
  and by value;
- `to_vec` copies them out, and `empty` builds a collection holding no script;
- `checkpoint`, `stage`, and `schema_hash` expose the other baked artifacts;
- `new` assembles a collection from `(script name, SQL)` pairs, and
  `from_migrations` wraps already parsed migrations; both sort the list
  deterministically, so a collection built by hand applies in the order a baked one
  does.

A script name identifies one migration. Two entries under one name would apply two
different bodies under a single recorded identity — the first is recorded, the
second is re-applied as a modified script and recorded again — so both constructors
refuse such a set with `Error::DuplicateScriptName`, and a run refuses a list
assembled by hand before it applies anything.

## Applying the baked collection

A run needs a connection. The example opens an in-memory SQLite database, and the
call that produces the connection is the one failure the migration error does not
carry, so it is translated into that error where it happens:

```rust,no_run
{{#rustdoc_include ../examples/migrations.rs:open}}
```

The protocol over that connection comes from the driver crate of the engine, as an
adapter that borrows the handle for the length of the run and hands it back when
the run finishes:

```rust,no_run
{{#rustdoc_include ../examples/migrations.rs:apply}}
```

The run reports what it applied and how long it took, which is what a startup logs
before it serves:

```rust,no_run
{{#rustdoc_include ../examples/migrations.rs:report}}
```

`migrate` performs the whole guarded lifecycle, and every step of it is switched on
by the artifact that was baked:

1. The history table is ensured under the run lock of the backend. A database that
   never ran a migration has none yet, and creating it is a schema write, so the
   creation happens under the same lock an applying run takes.
2. A target carrying no recorded history at all is initialised from the declarative
   stage when the collection bakes one. The cold start records a single marker
   instead of the DDL of the stage, and the marker sits at the newest version the
   collection declares — the higher of the newest tail version and the pruned
   boundary, `0` when it declares neither. Its digest is a catalog digest: it is
   the baked `schema.lock` one when the collection carries a snapshot, otherwise
   the boundary hash of a baked `checkpoint.toml`, and the digest of the stage
   text only when the collection bakes neither, so the counterpart tail replay
   has to produce the same catalog to agree with it. A
   database that already carries history is never cold-started.
3. A baked boundary marker arms the checkpoint guard: a target sitting below the
   pruned boundary is rejected instead of being advanced by a partial tail, and a
   target sitting exactly at the boundary is measured against the anchored hash.
4. The pending scripts are applied in run order: the versioned scripts in ascending
   version order, then the repeatable ones whose contents differ from the recorded
   digest.
5. The catalog the run produced is compared against the digest the baked
   `schema.lock` anchors, and a divergence is reported as
   `Error::SchemaHashMismatch` instead of being accepted as up to date. The
   digest is read under the same exclusive run lock as every other backend
   interaction of the run, so a concurrent lifecycle operation on the same
   database can neither drop nor advance the schema between the commit of the
   tail and the comparison.

A backend that cannot introspect itself reports no catalog, which leaves the
anchored digest unverified rather than rejecting a schema the backend never
described. A collection built by `new` or `from_migrations` bakes no artifact at
all, so it behaves exactly as the plain tail it describes: no cold start, no
boundary, and no final comparison.

## A failure before the first request

A run reports failure in a typed error of the migration crate, so a startup
distinguishes the cases instead of matching on text:

- `DuplicateScriptName` refuses a collection carrying two entries under one script
  name, before a connection is opened.
- `FailedMigrationAlreadyApplied` refuses a run over a history that records an
  unsuccessful attempt: the attempt has to be cleared first.
- `ChecksumMismatch` refuses an applied versioned script whose contents no longer
  match the recorded digest.
- `BaselineAlreadyExists` refuses a baseline on a database whose history already
  holds an entry.
- `MigrationExecutionFailed` reports a script that did not complete: its
  transaction is rolled back, the unsuccessful attempt is recorded, and nothing
  that follows it has run.
- `OutdatedCheckpoint` refuses a target below the pruned boundary instead of
  applying a partial tail.
- `CheckpointHashMismatch` reports a target sitting at the boundary whose schema no
  longer matches the anchored one.
- `SchemaHashMismatch` reports a run whose produced catalog diverges from the baked
  snapshot.
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

## The backend the run needs

Every operation takes the backend that performs the I/O. The collection opens no
connection of its own: it hands the scripts to the backend the caller supplies, so
the connection, the transaction, and the run lock are the ones the application
already controls.

A backend implements one contract, the same one the command line drives, and the
collection never names a driver. The protocol lives in the driver crate of each
engine, behind the feature that compiles it, and the facade enables that feature
for the drivers a build includes. The driver crate ships the protocol in the
shapes its engine allows:

- a standalone backend that owns its connection, for a tool that dials a database
  of its own;
- an adapter over a connection the application already owns, which borrows the
  handle for the length of the run — the shape the example uses, and the reason a
  service migrates through the very connection it queries with;
- for PostgreSQL, a blocking adapter over the asynchronous clients, which runs the
  asynchronous operations on a dedicated blocking thread and so keeps the
  capability of the asynchronous driver without asking the caller for a runtime.

The catalog digest a client backend records comes from the same introspection the
rest of the toolchain uses, so a cold start anchors the digest the snapshot and the
boundary marker carry whatever engine executed the stage. PostgreSQL names its
schema selection on the adapter, mirroring the standalone backend, so both record
the same catalog for the same project.

The migration crate ships an in-memory backend that its own runs and tests drive
without a server. An application that embeds migrations against a real database
supplies the backend that fits its driver: the collection carries the scripts, and
the service carries the connection.

## The command line and the embedded collection

During development the command line is the working loop: a new script is written,
the run applies what is pending, the status shows what is left, and a repair
clears a mistake. In production one binary starts, applies the scripts it carries,
and serves traffic. Both routes share the script naming grammar, the digest
algorithm, and the history table; they differ in what each one can reach.

Each route reads its artifact layout from the project configuration: `--config
<path>` names that file for a command, and the conventional `rurqe.toml` of the
working directory applies otherwise. The migration directory, the stage directory,
the boundary marker, and the catalog snapshot all resolve through it, so only the
configuration file decides where a command reads and writes.

| Concern | Command line | Embedded collection |
|---|---|---|
| Where the scripts come from | the migration directory the project configuration declares, read at run time | compiled into the binary, parsed at startup |
| Applying the pending scripts | `cargo rurqe migrate up` | `migrate` / `migrate_async` |
| Inspecting the status | `cargo rurqe migrate status` | `status` / `status_async` |
| Repairing the history | `cargo rurqe migrate repair` | `repair` / `repair_async` |
| Recording a baseline | `cargo rurqe baseline --version <version>` | `baseline` / `baseline_async` |
| Initialising an empty database from the declarative stage | `cargo rurqe stage init --version <version>` | performed by `migrate` when a stage is baked |
| Sealing a boundary | `cargo rurqe stage commit --version <version>` | not available |
| Verifying both descriptions | `cargo rurqe stage verify` | not available |
| Pruning the history | `cargo rurqe prune --before <version>` | not available |
| Sweeping abandoned scratch databases | `cargo rurqe stage cleanup` | not available |
| Outdated-target guard | armed by the boundary marker the project configuration resolves on `migrate up` | armed by the baked `checkpoint.toml` |

## What the embedded collection does not do

- It does not write artifacts. The boundary marker, the boundary snapshot, and the
  catalog snapshot are produced by the command line, so sealing a boundary and
  pruning the history stay command-line steps.
- It does not verify both descriptions of the schema. The stage and the replayed
  tail are compared by the command line alone, so the equivalence check of a
  project never runs inside a binary.
- It does not read files. The binary is the source of the run: a script edited on
  disk changes nothing until the binary is rebuilt, and the digests it records are
  computed from the text it executed.
- It does not open a connection and never names a driver: the backend is the
  caller's, and the collection holds no engine of its own.
