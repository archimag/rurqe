# Running migrations in production

An application can apply migrations during startup, before it accepts requests.
`embed_migrations!` bundles the project's reviewed SQL scripts into the binary
at build time. The running process needs database access, but does not need the
migration files on disk. A failed migration stops startup instead of serving
against an incompatible schema.

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

Rebuild the application whenever scripts are added or removed. Cargo tracks
the contents of files the macro included, but not the membership of their
directory. A build script can track the directory explicitly:

```text
// build.rs
fn main() {
    println!("cargo:rerun-if-changed=rurqe/migrations");
    println!("cargo:rerun-if-changed=rurqe/schema");
}
```

Adjust those paths to the directories your configuration resolves. Deploy the
new binary with its embedded scripts; changing a SQL file on the server does
not change what an already built binary will run.

## Migrate before serving requests

This compiled example builds a PostgreSQL pool, applies the embedded migration
set, then makes a query through that pool. In a service, the query marks the
point where request handling can begin:

```rust,no_run
{{#rustdoc_include ../examples/migrations.rs:startup}}
```

The example pool reads `DATABASE_URL` (or uses a local development endpoint).
Configure the application's real pool, credentials, TLS, and schema selection
for the deployment; the migration adapter must select the same schemas as the
project's catalog snapshot, using `with_schemas` when the project does not use
the default `public` schema. Do not use the example's local endpoint as a
production connection policy.

Creating `PgAsyncMigrationBackend` waits for a worker and a pool checkout, so
the example creates it with `spawn_blocking`. `migrate_async` then runs the
blocking migration protocol off the Tokio worker thread. The backend holds one
session for the entire run: PostgreSQL's migration advisory lock belongs to
that session and is released before it returns to the pool. Do not call the
synchronous `migrate` method on an async worker.

The returned pair contains the migration result and the backend. On an
ordinary migration failure, the result is an error and startup returns without
running the query. The backend is returned for inspection or recovery when the
blocking task completes; if the task itself fails, it returns no backend. The
example drops it after success to release its dedicated worker and checkout.

An empty database is initialized from the declarative stage if one was baked.
Otherwise the versioned tail runs in version order, followed by changed
repeatable scripts. A baked checkpoint refuses a database older than its
pruned boundary; a baked `schema.lock` checks the resulting catalog. An
already migrated database only applies pending changes. Keep the application
closed to traffic until this step succeeds on every startup.

## Failed deployments

Do not automatically repair a failed production migration or rewrite an
already deployed script. Inspect the error and recorded history, fix the
problem with a reviewed follow-up migration, and deploy a new binary. A
non-transactional script may have committed some work before failing: inspect
the database state before any deliberate `repair`. A failed startup must not
continue to serve requests with a partially migrated database.
