# Embedded migrations in an application

# Embedded migrations in an application

`embed_migrations!` bundles reviewed SQL scripts into the application binary
at build time. The application needs database access at runtime, but not the
SQL files on disk. Check the migration state and apply pending changes before
the application begins using the database; stop startup on an error.

## Prepare the release

Keep the migration scripts and `rurqe.toml` in the application project. For
example, a configuration can declare the directory containing versioned and
repeatable scripts:

```toml
[migrations]
dir = "migrations"
```

Enable the `macros`, `migrate`, `deadpool-postgres`, and `tokio` features of
`rurqe`; the application also needs Tokio and its PostgreSQL pool. The
`embed_migrations!` argument names the configuration file relative to the
application crate's manifest directory. Without an argument, the macro looks
for `rurqe.toml` above that directory. The example uses a test fixture as its
configuration so it compiles in this repository; an application names its own
file, for example `embed_migrations!("rurqe.toml")?`.

Build and deploy a new binary after changing the migration scripts. Adding or
removing a script also requires a rebuild so that the embedded collection
reflects the directory's new contents. Changing files on a deployed machine
does not change an already built binary.

## Include migrations in the application

Create a PostgreSQL pool for the database the application uses, and embed the
project's migration artifacts. The example names a repository fixture so it
compiles without connecting to a database during documentation checks; replace
that path with the application's `rurqe.toml`:

```rust,no_run
{{#rustdoc_include ../examples/migrations.rs:setup}}
```

The example pool reads `DATABASE_URL` or uses a local development endpoint.
Configure credentials, TLS and schema selection for your deployment. When the
project uses schemas other than `public`, select the same schemas in the
migration backend with `with_schemas`.

Constructing the backend waits for a pool checkout, so the example does it in
`spawn_blocking`. The `*_async` methods then run the blocking migration
protocol off the Tokio worker thread. They return a pair: the operation's
`Result` and an optional backend for a subsequent operation. A task that
failed before returning cannot return its backend.

## Inspect migration status

`status_async` returns one `MigrationStatus` per embedded script. Check the
result before applying changes:

```rust,no_run
{{#rustdoc_include ../examples/migrations.rs:status}}
```

`Pending` means a script has not run; `Applied` matches the recorded checksum.
`IgnoredBaseline` means a versioned script is at or below the recorded baseline.
`Modified` means an applied script differs from its recorded version; `Failed`
means a previous run failed. The example stops on `Modified` or `Failed` for
investigation rather than attempting to repair or override history. Status
may create the migration-history table on a database that has none; it does not
apply the scripts or validate the final catalog snapshot.

## Set a baseline for an existing database

Baseline is a **separate, one-time operation**, not part of normal startup.
Use it only when the database already has the schema represented by the
selected version. Verify that schema and choose the corresponding version
before recording it; a baseline records history but does not create or inspect
the application tables. Do not baseline an empty database to skip its scripts.

```rust,no_run
{{#rustdoc_include ../examples/migrations.rs:baseline}}
```

`baseline_async` returns a `SchemaHistoryEntry` containing the recorded
version and installation rank. Keep that entry as the audit record of what was
marked as already present. A second baseline is refused if history already
contains an entry. After this one-time operation, inspect status again: scripts
at or below the baseline are ignored, and later scripts remain pending.

## Apply pending migrations

Run `migrate_async` before the application starts using the database:

```rust,no_run
{{#rustdoc_include ../examples/migrations.rs:apply}}
```

`MigrationReport::applied_count` and `applied_scripts` identify the tail scripts
run in this invocation. `cold_start` reports the declarative stage used to
initialize an empty database, if one was embedded. A count of zero is normal
when the database is already current. An error stops the example before the
application uses the pool; inspect the error and history instead of continuing.

An empty database is initialized from the declarative stage if one was baked.
Otherwise the versioned tail runs in version order, followed by changed
repeatable scripts. A baked checkpoint refuses a database older than its
pruned boundary; a baked `schema.lock` checks the resulting catalog. An
already migrated database only applies pending changes.

## Failed deployments

Do not automatically repair a failed production migration or rewrite an
already deployed script. Inspect the error and recorded history, fix the
problem with a reviewed follow-up migration, and deploy a new binary. A
non-transactional script may have committed some work before failing: inspect
the database state before any deliberate `repair`. Do not continue using a
database whose migration failed.
