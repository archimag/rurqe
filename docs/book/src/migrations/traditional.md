# The traditional path: `V...` and `R...`

The classic Flyway-compatible cycle is fully implemented in `rurqe-migrate` and is available both through the CLI and as an embedded runner.

## Script naming

`Migration::parse(script, sql)` parses the name and immediately computes `Checksum::of(sql.as_bytes())`.

| Name | Parsed as |
|---|---|
| `V2_1__add_orders.sql` | `Versioned`, version `2.1`, description `add orders` |
| `V1.0.0__init.sql` | `Versioned`, version `1.0.0` (dots and underscores are equivalent) |
| `R__views_and_funcs.sql` | `Repeatable`, no version, description `views and funcs` |

Rules the parser enforces:

- the extension must be `.sql`;
- a versioned name is the `V` prefix, then a non-empty version, then `__`;
- a repeatable name is the `R` prefix followed immediately by `__`;
- underscores in the description are replaced by spaces (that is how they reach the history);
- a version is a sequence of numeric parts separated by `.` or `_`; the order is numeric, so `1.2 < 1.10` and `1 < 1.0`.

A violation yields `Error::InvalidScriptName` or `Error::InvalidVersion` — exactly what has to be fixed.

## Transactionality

A script is transactional unless stated otherwise. Opting out has two equivalent forms:

1. The name: `V50__rebuild_index.notx.sql` or `V50__rebuild_index__notx.sql` (the `notx.sql` suffix, case-insensitive).
2. A directive in a comment occupying a whole line:

```sql
-- executeInTransaction = false
CREATE INDEX CONCURRENTLY users_email_idx ON users (email);
```

Supported spellings of the key are `executeInTransaction`, `execute_in_transaction` and `transactional`; the value is `false`. The comment scanner (`comment_text`) tracks string literals, quoted identifiers, `$$ … $$` dollar-quoted bodies and nested block comments, therefore:

- `-- transactional = false` inside a `plpgsql` function body is data, not a directive;
- a directive inside a nested comment is ignored, while one in the outer comment works;
- mentioning the switch in prose ("set transactional = false here when…") does not disable the transaction: the line must be a directive and nothing else;
- an unterminated literal or comment does not panic — the scanner stays in a consistent state.

## Applying

`Migrator::migrate` works as follows:

1. Acquire the run lock (`LockGuard::acquire`) and ensure the history table exists.
2. Validate version uniqueness (`DuplicateMigrationVersion`).
3. Read and validate the history: a failed entry → `FailedMigrationAlreadyApplied`; a modified versioned script → `ChecksumMismatch`.
4. Sort the scripts: versioned ones by ascending version, then repeatable ones by name.
5. Decide per script whether to apply it: a version below or equal to `baseline` is skipped; an already recorded one is skipped; a repeatable script is applied when its checksum differs from the last recorded one.
6. Apply it and write the history row (`SchemaHistoryEntry::applied`) in the same transaction.

The order of the application details (`runner/apply.rs`):

| Script kind | What happens |
|---|---|
| transactional | `begin` → execute the SQL → write the applied row → `commit`; on error `rollback`, best-effort write of a failed row, `MigrationExecutionFailed` |
| non-transactional | first the in-progress row is committed (`success = false`), then the script runs outside a transaction |

The non-transactional path has three outcomes besides success:

- the backend refused before execution (`NonTransactionalUnsupported`) → the in-progress row is revoked and nothing remains in the history;
- the script failed → the in-progress row stays as a marker of the attempt, and the next attempt requires `repair`;
- the script applied but the applied row could not be written → `HistoryNotRecorded`: the schema has already changed and the history lags behind, so `repair` or `baseline` is required.

## Commands

```bash
cargo db-migrate up       [--migrations-dir <dir>] [--checkpoint-path <path>] \
                          [--database-url <url>] [--schema <name>]
cargo db-migrate status   [--migrations-dir <dir>] [--database-url <url>] [--schema <name>]
cargo db-migrate repair   [--migrations-dir <dir>] [--database-url <url>] [--schema <name>]
cargo db-baseline --version <version> [--description <desc>]
```

- `up` — the default when no action is given. Only this command accepts `--checkpoint-path`: it arms the outdated-target guard.
- `status` reads the history and prints a `SCRIPT | KIND | VERSION | STATUS` table; `status` on an uninitialised database creates the history table itself, so the table check and the read happen under the same run lock as the run.
- `repair` deletes failed entries and rewrites the checksums of modified versioned scripts.
- `baseline` writes a `SchemaHistoryEntry::baseline` row (kind `Baseline`, no checksum) and refuses to work if the history is already non-empty (`BaselineAlreadyExists`).

## The embedded runner

To run from an application without the CLI:

```rust
use rurqe_migrate::{embed_migrations, InMemoryBackend};

let embedded = embed_migrations!(
    "V1__init.sql" => "CREATE TABLE users (id INT);",
    "R__view.sql" => "CREATE VIEW users_v AS SELECT * FROM users;",
).expect("valid migrations");

let mut backend = InMemoryBackend::new();
let report = embedded.migrate(&mut backend).expect("migration succeeds");
assert_eq!(report.applied_count, 2);
```

`EmbeddedMigrations::new` sorts the scripts with the same `Migrator::sort_migrations`, so the embedded path and the CLI see the same order. The synchronous methods block the thread and therefore check the execution context through `guard_blocking_entry`: by default they warn, and under the `strict-async` feature they refuse to run on a Tokio thread. The asynchronous wrappers (`migrate_async`, `status_async`, `repair_async`, `baseline_async`, under the `tokio` feature) perform the same work on `spawn_blocking` and hand the backend back to the caller.

## CLI connectivity

This build of `rurqe-cli` does not link a TLS backend, so the transport policy is enforced before the driver (`enforce_transport_policy`):

- `sslmode=require` and `verify-ca`/`verify-full` are rejected on any host;
- a remote host is accepted only with an explicit `sslmode=disable`;
- loopback and Unix sockets are accepted without specifying a mode.

This matters for `migrate`, because the database password is passed in the connection string itself.

Next: [Stage + Tail](stage_tail.md).
