# Schema Evolution: Overview

Schema maintenance is a lifecycle of its own, decoupled from the rest of the library: query code (`rurqe-core`, `rurqe-postgres`, `rurqe-sqlite`) knows nothing about migrations and never imports them. The implementation of the schema lifecycle is collected in two places:

- `rurqe-migrate` — the script model (`Migration`, `MigrationKind`, `MigrationVersion`, `Checksum`), the history table (`SchemaHistoryEntry`, `SCHEMA_HISTORY_TABLE`), the runner (`Migrator`, `MigrationBackend`, `MigrationTransaction`, `LockGuard`), the declarative stage (`DeclarativeStage`, `StageScript`, `StageMarker`, `cold_start`) and the boundary marker (`Checkpoint`).
- `rurqe-cli` — the commands (`migrate`, `baseline`, `codegen`, `verify`, `stage init|verify|commit|cleanup`, `prune`) and the checkpoint artifact protocol: the serialising `checkpoint.lock` section, the crash-safe installation of the "marker + boundary snapshot" pair, and the recovery of an interrupted commit.

These two layers are joined by a single format — the catalog snapshot (`SchemaCatalog` → `schema.lock` and `checkpoint.toml`) — not by shared code.

## Two modes of one runner

The history runner is the same in both modes: `Migrator` reads `__rurqe_schema_history`, validates checksums and applies the tail. The second mode is not a different runner but a different way of creating the database that this runner then advances.

| Property | Traditional (`migrations/`) | Stage + Tail (`schema/` + `migrations/`) |
|---|---|---|
| Source of truth | an ordered set of scripts | the declarative stage plus the active tail |
| Initialising a clean database | replay every `V...` | `stage init` → `cold_start` from `schema/` |
| Advancing a live database | `migrate up` | `migrate up --checkpoint-path checkpoint.toml` |
| Removing old migrations | squash via `baseline` | `prune --before <version>` without a squash |
| Integrity check | `db-verify` (drift against `schema.lock`) | `stage verify` (Golden Equation) |
| History footprint | one row per script | one `<< Stage >>` entry plus the tail rows |

Both models can coexist in one project: `cold_start` records a marker of kind `Baseline`, so every `V...` version at or below the marker version counts as already applied (reported as `IgnoredBaseline` in `status`), while the active tail above the marker keeps being applied by the ordinary `migrate up`.

## Section contents

- [The crisis of traditional migrations in the AI era](problem.md) — why a linear script history stopped scaling.
- [Two modes and how to choose](modes.md) — what the modes share and where exactly they differ.
- [The traditional path: `V...` and `R...`](traditional.md) — naming, checksums, transactionality, `migrate`/`baseline`/`repair`.
- [Stage + Tail: declarative stage and cold start](stage_tail.md) — `schema/*.sql`, `StageMarker`, instant initialisation, boundary commit.
- [The Golden Equation](golden_equation.md) — reconciling `State(schema/)` against `State(Checkpoint + tail)`.
- [Safe history pruning](pruning.md) — `prune --before` and the outdated-target guard.
- [The checkpoint protocol](checkpoint_protocol.md) — `checkpoint.toml`, `checkpoint.lock`, the owner record, recovery.

## Shared primitives

| Primitive | Declared in | Role |
|---|---|---|
| `Migration` | `rurqe-migrate/src/migration.rs` | a parsed script: kind, version, description, SHA-256, transactionality flag |
| `MigrationVersion` | `rurqe-migrate/src/version.rs` | the version as `Vec<u64>`, with a total numeric order |
| `Checksum` | `rurqe-migrate/src/checksum.rs` | SHA-256 in hex, comparing the recorded value against the actual one |
| `SchemaHistoryEntry` | `rurqe-migrate/src/history.rs` | one `__rurqe_schema_history` row |
| `MigrationBackend` / `MigrationTransaction` | `rurqe-migrate/src/runner/backend.rs` | driver contracts: history, lock, transactions |
| `LockGuard` | `rurqe-migrate/src/runner/guard.rs` | RAII holding of the run lock |
| `DeclarativeStage` / `StageScript` | `rurqe-migrate/src/stage/script.rs` | the declarative `schema/` |
| `StageMarker` | `rurqe-migrate/src/stage/marker.rs` | the synthetic stage marker entry |
| `Checkpoint` | `rurqe-migrate/src/checkpoint.rs` | the pruning boundary and the stage hash at that boundary |
| `GoldenEquation` / `GoldenReport` | `rurqe-cli/src/stage/equation.rs` | reconciling the stage against the checkpoint and the tail |

## The history table

Both models write into one table, created by `create_schema_history_table_sql()`:

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

The `type` column holds the text of `MigrationKind::as_str()`: `VERSIONED`, `REPEATABLE` or `BASELINE`. The `success` column distinguishes an applied entry from the marker of a started or failed attempt, and a missing `checksum` (a `baseline` entry) means there is nothing to compare, so the entry is never considered modified (`SchemaHistoryEntry::is_modified`).

## How to read this section

Every statement in this section is derived from the sources of `crates/rurqe-migrate` and `crates/rurqe-cli/src/{cli,stage}` and from their unit tests. `rurqe.toml` carries no key that enables a mode: `CodegenConfig::parse` rejects any unknown section and any unknown key, and it knows exactly `schema`, `[codegen]`, `[codegen].domain_types` and `[codegen].type_overrides`. The mode is determined by the set of directories and by the set of commands, not by a configuration flag.
