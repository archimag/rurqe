# Stage + Tail: declarative stage and cold start

The second model adds a source of truth describing what the schema should be. A live database is still advanced by scripts, but a clean database is created from the declaration.

## Directories

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

## `DeclarativeStage`

The stage is simply a directory of `*.sql` files:

- `DeclarativeStage::DIRECTORY == "schema"` is the default name (overridden by `--stage-dir`);
- `DeclarativeStage::load(dir)` returns `Ok(None)` when the directory does not exist and an error when a file cannot be read (silently skipping a script would initialise the database with an incomplete schema);
- scripts are sorted by name, so the application order does not depend on the filesystem traversal order;
- transactionality is determined by the same attribute parser as for migrations (`StageScript::new` → `parse_script_attributes`);
- `DeclarativeStage::checksum()` is the SHA-256 of the concatenation of `name + \n + sql + \n` in application order: reordering files does not change the digest, editing any script does.

## Cold start

`cold_start(backend, stage, marker)` creates a database from scratch:

1. Acquire the run lock and ensure the history table exists.
2. Read the history and look for the last `<< Stage >>` entry:
   - if the digest of the catalog the backend actually sees (`guard.catalog_checksum()`) matches the recorded one and the versions match → the command reports "already initialised" and does nothing;
   - if the digest diverged → `Error::InvalidStage`, pointing at reinitialising the database or committing the current stage first;
   - if a failed (in-progress) `<< Stage >>` entry is found → `Error::InvalidStage` about an interrupted cold start;
   - if the history is non-empty with anything else → `Error::BaselineAlreadyExists`.
3. If the stage contains non-transactional scripts, commit the in-progress marker (`success = false`) up front, so a failure in the middle of the run is diagnosable.
4. Apply the stage: consecutive transactional scripts in one transaction, each non-transactional script separately.
5. Compute the resulting catalog digest, write the applied marker (`success = true`) and, if an in-progress marker existed, delete it in the same transaction.

The returned `ColdStartReport` carries `stage_scripts` (names of the applied files), `installed_rank` and `total_time_ms`.

### What exactly reaches the history

`StageMarker` is written as an ordinary history row:

| Field | Value |
|---|---|
| `version` | the version passed to `--version` |
| `description` | `--description` (defaults to `stage`) |
| `kind` | `MigrationKind::Baseline` |
| `script` | `STAGE_SCRIPT == "<< Stage >>"` (the constant `<< Stage >>`) |
| `checksum` | the catalog digest, not the digest of the DDL text |
| `execution_time` | 0 |

The version of this marker is the boundary: everything at or below it is considered passed by the runner.

An important detail: the marker anchors the *canonical* catalog digest (`SchemaCatalog::checksum()`), which the backend derives by introspecting what was actually created. The marker therefore matches a `checkpoint.toml` produced by `stage commit` regardless of the engine. A backend that cannot introspect itself stays with the digest passed in from the stage.

## Commands

```bash
cargo rurqe stage init --version <version> [--stage-dir <dir>] [--description <desc>] \
                       [--database-url <url>] [--schema <name>]
```

An empty stage is rejected ("holds no `*.sql` script to apply"), and on an already initialised database the command reports "Database already initialised at stage X; nothing to apply".

```bash
cargo rurqe stage commit --version <version> [--stage-dir <dir>] [--lock-path <path>] \
                         [--checkpoint-path <path>] [--database-url <url>] [--schema <name>]
```

`stage commit` is the seam between the two models. It:

1. loads the stage and introspects the live catalog;
2. runs the parity gate: `compare_stage_with_live` materialises the stage in a scratch database and compares the catalogs. A divergence → an error listing the findings and reminding that the gate fired *before* the checkpoint section, so no artifact was touched;
3. renders `schema.lock` (`catalog.to_lock()`) and `Checkpoint::new(version, catalog.checksum())`;
4. assembles the boundary snapshot (`<stem>-stage/`) in a temporary directory next to the target and syncs it;
5. enters the `checkpoint.lock` section, sweeps the garbage of previous attempts, recovers an interrupted commit, checks the no-rollback rule (the checkpoint version cannot be lowered), writes `schema.lock` and performs `seal_commit`;
6. reports: the recovery (if any), the "vacuous" schema-selection warning (if any) and the swap notices.

The details of steps 5 and 6 are in [The checkpoint protocol](checkpoint_protocol.md).

## Scratch databases

Materialisation runs in a separate database so that the declarative DDL is never executed against the live catalog:

- PostgreSQL: the same URL is used, and the materialisation creates a disposable scratch database;
- SQLite: `:memory:`, while the URL points at the very file the commit seals.

Orphaned PostgreSQL scratch databases are removed by `stage cleanup` — only databases tagged by this host, older than `--older-than` and not owned by a live process; without `--yes` the command merely lists the candidates.

Next: [The Golden Equation](golden_equation.md).
