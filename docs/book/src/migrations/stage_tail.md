# Stage + Tail: the declarative stage and cold start

The second model adds a source of truth describing what the schema should be. A live database is still advanced by scripts; a clean database is created from the declaration.

## What it buys you

- **A clean database in one step.** CI, a throwaway container, a local reset: `stage init` applies `schema/*.sql` once and nothing historical is replayed.
- **One file to read.** A newcomer or an LLM opens `schema/users.sql` instead of fifty `V...` files — the structure is reviewed as a declaration, not as an archaeology dig.
- **A tail that only carries the delta.** Data migrations and structural changes for already deployed databases stay in `migrations/`, short and current.

## Layout

```text
project/
├── schema/                     # declarative stage (SSOT of the structure)
│   ├── users.sql
│   └── billing.sql
├── migrations/                 # active tail
│   ├── checkpoint.toml         # pruning boundary
│   ├── checkpoint-stage/       # verbatim stage snapshot at the boundary
│   ├── checkpoint.lock         # writer serialisation section
│   └── V051__add_credits.sql
├── schema.lock                 # deterministic catalog snapshot
└── rurqe.toml
```

## The stage is a directory of SQL

- `schema` is the default directory name (`DeclarativeStage::DIRECTORY`); `--stage-dir` overrides it.
- `DeclarativeStage::load` returns `Ok(None)` when the directory is absent — "this project has no declarative stage" — and an error when a file cannot be read: silently skipping a script would initialise the database with an incomplete schema.
- Scripts are sorted by name, so the applied DDL does not depend on the file system traversal order.
- Transactionality comes from the same attribute parser the migration tail uses (`*.notx.sql` in the name or the directive in a comment), so a statement the engine refuses inside a transaction runs where it belongs.
- `DeclarativeStage::checksum()` hashes `name + "\n" + sql + "\n"` in application order: reordering files never changes the digest, editing one always does.

## Cold start

`cold_start(backend, stage, marker)` builds the database from the declaration:

1. Acquire the run lock and ensure the schema history table exists.
2. Read the history and look for the last applied `<< Stage >>` marker:
   - same version and same catalog digest → the run is a no-op;
   - diverged digest → `InvalidStage`, because the database was initialised from a stage that no longer matches the one on disk;
   - an in-progress (`success = false`) marker → `InvalidStage` about an interrupted cold start: `repair` clears the marker, and the database itself is re-initialised from scratch;
   - any other history → `BaselineAlreadyExists`.
3. If the stage holds non-transactional scripts, commit the in-progress marker first: such a stage cannot be applied atomically, and a run that dies midway must stay visible in the history.
4. Apply the stage — consecutive transactional scripts share one transaction, each non-transactional script runs on its own.
5. Introspect the resulting catalog, write the applied marker and drop the in-progress one in the same transaction.

The run returns a `ColdStartReport` with the applied script names, the rank of the marker and the elapsed time.

The checksum the marker anchors is the digest of the catalog the stage actually produced, not of the DDL text, so the row matches the `checkpoint.toml` that `stage commit` writes whatever engine executed the DDL. A backend that cannot introspect itself keeps the digest supplied with the marker.

An asynchronous startup path uses `cold_start_async` (feature `tokio`), which runs the same work on a blocking thread. The blocking guard warns by default and refuses a cold start started on an async worker thread under the `strict-async` feature.

## Commands

```bash
cargo rurqe stage init --version <version> [--stage-dir <dir>] [--description <desc>] \
                       [--database-url <url>] [--schema <name>]
```

`--version` is required and `--description` defaults to `stage`. An empty stage is rejected ("holds no `*.sql` script to apply"), and an already initialised database reports "Database already initialised at stage X; nothing to apply".

```bash
cargo rurqe stage commit --version <version> [--stage-dir <dir>] [--lock-path <path>] \
                         [--checkpoint-path <path>] [--database-url <url>] [--schema <name>]
```

`stage commit` is the seam between the two models. It:

1. loads the stage and introspects the live catalog;
2. runs the parity gate — `compare_stage_with_live` materialises the stage in a scratch database and compares the catalogs; a divergence fails the command and lists the findings, and because the gate fires before the checkpoint section, no artifact has been touched yet;
3. renders `schema.lock` and `Checkpoint::new(version, catalog.checksum())`;
4. assembles the boundary snapshot (`<stem>-stage/`) in a temporary directory next to the target and syncs it;
5. enters the `checkpoint.lock` section, sweeps the garbage of previous attempts, recovers an interrupted commit, refuses to lower the checkpoint version, writes `schema.lock` and seals the commit;
6. reports the recovery (if any), the "vacuous" schema-selection warning (if any) and the swap notices.

The details of steps 5 and 6 are in [The checkpoint protocol](checkpoint_protocol.md).

## Scratch databases

Materialisation never touches the live catalog:

- PostgreSQL — the same server is used, and the materialisation creates a disposable scratch database;
- SQLite — `:memory:` is used, while the URL points at the very file the commit seals.

Abandoned PostgreSQL scratch databases are removed by `stage cleanup`, which only lists candidates unless `--yes` (or `--force`) confirms, only considers databases tagged by this host, and skips any whose recorded creating process may still be alive.

```bash
cargo rurqe stage cleanup [--scratch-url <url>] [--database-url <url>] \
                          [--older-than <seconds>] [--yes]
```

`--older-than` changes the age above which a database counts as abandoned.

Next: [The Golden Equation](golden_equation.md).
