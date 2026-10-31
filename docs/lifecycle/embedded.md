# Running migrations in production

A migration reaches a database from one of two places. The command line reads the
scripts from `migrations/` and applies them as a process of its own. An
application binary carries the same scripts inside itself and applies them while
it starts. The second arrangement is the one a container needs: no scripts
directory has to be mounted into the image, no file has to be readable at run
time, and the scripts a binary runs are exactly the scripts it was built from.
This chapter calls that arrangement embedded migrations.

The scripts and the record do not change between the two routes. An embedded
migration carries the same file name, the same version grammar, and the same
SHA-256 digest as the directory holds, and it is recorded in the same history
table with the same kind and the same rank. A database advanced by a container
start ends exactly where a command-line run over the same files would have left
it.

## Embedding the scripts

`embed_migrations!` takes pairs of script name and script text and yields a
fallible collection:

```rust,no_run
{{#rustdoc_include ../examples/migrations.rs:embed}}
```

Every pair is parsed and digested when the collection is built, so a script whose
name follows no convention, and a version that does not parse, stop the process at
startup instead of leaving a gap in the recorded history. The collection is sorted
once, the way a directory of scripts is sorted: versioned scripts in ascending
version order, followed by the repeatable scripts ordered by script name.

The macro accepts two spellings of a pair — `"<name>" => "<sql>"` and
`("<name>", "<sql>")` — and a call without a pair yields an empty collection.
Both spellings reach the same constructor, so the choice is a matter of how the
script reads beside the code that carries it.

## The embedded collection

`EmbeddedMigrations` holds the parsed scripts and exposes them without hiding what
they are:

- `len` and `is_empty` state how many scripts the binary carries;
- `migrations` returns them as a slice, and the collection iterates by reference
  and by value;
- `to_vec` copies them out;
- `empty` builds a collection holding no script;
- `from_migrations` wraps already parsed migrations and sorts them
  deterministically.

## Applying the collection

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
— and returns how many scripts ran together with their names. A run that finds
nothing pending applies nothing and leaves the database untouched. A database that
never ran a migration holds no history table yet, and the run creates it under the
same run lock the command line takes.

`status` executes no script and returns one entry per script, carrying the script
name, its kind, its version, and its status: `Pending` for a script never applied,
`Applied` for one whose recorded digest matches, `Modified` for one that changed
after it ran, `IgnoredBaseline` for one below the version a baseline recorded, and
`Failed` for an attempt that did not complete.

`repair` executes no script either. It purges every unsuccessful entry and
rewrites the recorded digest of an applied versioned script whose contents
changed, and it reports what it removed and which digests it rewrote.

`baseline` records a starting point for a database that already carries the
schema. It takes the version and the description of the entry, and a database
whose history already holds a migration or a baseline at that version is refused
instead of silently restarted.

Every operation takes the backend that performs the I/O. The collection opens no
connection of its own: it hands the scripts to the backend the caller supplies, so
the connection, the transaction, and the run lock are the ones the application
already controls.

The asynchronous counterparts take that backend by value and hand it back together
with the outcome, so a connection that cannot be shared across threads stays
usable after the run. A run that fails keeps the connection inside the finished
task, so an application that has to continue on the same connection after a
failure either opens a new one or uses the blocking method.

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

Because the scripts are baked in, the binary and the scripts it runs cannot drift
apart, and the digests it records are computed from the text it executed. A script
edited after the binary was built reaches the database only once the binary is
rebuilt.

## The command line and the embedded collection

During development the command line is the working loop: a new script is written,
the run applies what is pending, the status shows what is left, and a repair
clears a mistake. In production one binary starts, applies the scripts it carries,
and serves traffic. Both routes share the script naming grammar, the digest
algorithm, and the history table; they differ in what each one can reach.

| Concern | Command line | Embedded collection |
|---|---|---|
| Where the scripts come from | `migrations/` on disk, read at run time | compiled into the binary, parsed at startup |
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
- It does not open connections, and it ships no backend for a real database. The
  toolchain ships a PostgreSQL backend and a SQLite backend for its own runs, and
  the migration crate ships an in-memory backend for tests, so an application that
  embeds migrations against a real database implements the same backend contract
  over the connection it opens.
