# Two modes and how to choose

Both models share the same runner, the same history table and the same checksum format. Only the source of truth and the initialisation path differ.

## The shared part

| What | Shared across modes |
|---|---|
| Runner | `Migrator` (`migrate`, `status`, `repair`, `baseline`) |
| History table | `__rurqe_schema_history` |
| Naming format | `V<version>__<description>.sql`, `R__<description>.sql` |
| Checksum | SHA-256 of the script contents |
| Transactionality | the `*.notx.sql` name or a directive in a comment |
| Run lock | `LockGuard` → `MigrationBackend::acquire_lock` |
| Advancing a live database | `cargo db-migrate up` (or `cargo rurqe migrate up`) |

The differences start where a project acquires a declarative `schema/` and a `checkpoint.toml` marker.

## Comparison

| Aspect | Traditional | Stage + Tail |
|---|---|---|
| Directories | `migrations/` | `schema/` + `migrations/` + `schema.lock` + `checkpoint.toml` + `<stem>-stage/` |
| Initialising a clean database | `migrate up` applies everything | `stage init --version X` → `cold_start` |
| Initialisation cost | O(number of migrations) | O(current schema) |
| Source of truth | script order | contents of `schema/` |
| Divergence control | `db-verify` against `schema.lock` | `stage verify` (Golden Equation) |
| Shortening the history | `baseline` (squash) | `prune --before` |
| Protection of an old database | none | `Checkpoint::validate_history` (`OutdatedCheckpoint`) |
| Artifacts in git | `migrations/*.sql` | `schema/*.sql`, `migrations/*.sql`, `schema.lock`, `checkpoint.toml`, `<stem>-stage/*.sql` |

## The mode is not a configuration key

`rurqe.toml` carries no mode switch: the `CodegenConfig::parse` parser accepts only the `schema` key and the `[codegen]` section, and rejects any other section or key as an error. The mode is defined by the actual composition of the project and by the set of commands:

- only `migrations/` present → the traditional cycle `migrate up` / `baseline` / `repair` works;
- `schema/` present → `stage init`, `stage commit`, `stage verify`, `stage cleanup` are available;
- `migrations/checkpoint.toml` present → `migrate up --checkpoint-path …` enables the outdated-target guard, and `prune` is allowed to delete scripts.

Nothing prevents enabling the second mode later: `stage init` on an existing database is rejected (`Error::BaselineAlreadyExists`, or `Error::InvalidStage` when the digest diverges), so the migration between modes goes through `stage commit`, which records the live catalog as the boundary without replaying the history.

## The mixed transition

After `stage init` (a cold start) the history keeps a single `<< Stage >>` entry of kind `Baseline` with version `X`. The runner treats it as follows:

- every `V...` with a version `<= X` counts as already passed and is reported as `IgnoredBaseline` in `status`;
- every `V...` with a version `> X` is applied in the ordinary way;
- `R...` scripts are applied whenever their checksum differs from the recorded one.

This is exactly the semantics that lets a project with a history keep using its existing `migrations/` files without rewriting them.

## How to choose

| Situation | Recommendation |
|---|---|
| A historical database whose process cannot change | traditional mode |
| A new project with frequent clean initialisations in CI | Stage + Tail |
| Many DML migrations carrying data | Stage + Tail: `schema/` for structure, the tail for data |
| A CI check that "schema == checkpoint + tail" | Stage + Tail plus `stage verify` |

Next: [The traditional path](traditional.md) and [Stage + Tail](stage_tail.md).
