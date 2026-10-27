# Two modes and how to choose

Both models run through the same runner, write the same history table and follow the same naming and checksum rules. Only the source of truth and the initialisation path differ.

## What you get either way

- `cargo db-migrate up` advances a live database; `cargo rurqe migrate up` does the same through the workspace entrypoint.
- Names follow `V<version>__<description>.sql` and `R__<description>.sql`, and the checksum is the SHA-256 of the script contents.
- A script named `*.notx.sql` — or carrying the matching directive in its header comment — runs outside a transaction.
- `status` and `repair` behave identically, and every run holds the run lock through `LockGuard` → `MigrationBackend::acquire_lock`.

The differences start where a project acquires a declarative `schema/` and a `checkpoint.toml` boundary.

## Side by side

| Aspect | Traditional | Stage + Tail |
|---|---|---|
| Directories | `migrations/` | `schema/` + `migrations/` + `schema.lock` + `checkpoint.toml` + `<stem>-stage/` |
| Clean database | `migrate up` replays every script | `stage init --version X` applies the declaration once |
| Cost of initialisation | O(number of migrations) | O(current schema) |
| Source of truth | script order | contents of `schema/` |
| Divergence control | `db-verify` against `schema.lock` | `stage verify` (the Golden Equation) |
| Shortening the history | `baseline` (squash) | `prune --before` |
| Protection of an old database | none | `Checkpoint::validate_history` rejects it as outdated |
| Artifacts in git | `migrations/*.sql` | the above plus `schema/*.sql`, `schema.lock`, `checkpoint.toml`, `<stem>-stage/*.sql` |

## The mode is not a configuration key

`rurqe.toml` carries no mode switch: `CodegenConfig::parse` accepts the `schema` key and the `[codegen]` section and rejects any other section or key as an error. The mode is defined by what the project actually contains and by the set of commands:

- only `migrations/` present → the traditional cycle `migrate up` / `baseline` / `repair` works;
- `schema/` present → `stage init`, `stage commit`, `stage verify`, `stage cleanup` are available;
- `migrations/checkpoint.toml` present → `migrate up --checkpoint-path …` arms the outdated-target guard, and `prune` is allowed to delete scripts.

## Switching later

Nothing prevents adopting the second mode later. A cold start never overwrites an initialised database: `stage init` on a database that already carries history fails with `Error::BaselineAlreadyExists`, or with `Error::InvalidStage` when a stage marker exists but its digest diverged from the stage on disk. The supported way across is `stage commit`, which records the live catalog as the boundary without replaying anything.

## The mixed transition

After a cold start the history holds a single `<< Stage >>` entry of kind `Baseline` with version `X`. The runner treats it as follows:

- every `V...` with a version `<= X` counts as already passed and shows up as `IgnoredBaseline` in `status`;
- every `V...` above `X` is applied in the ordinary way;
- every `R...` is applied whenever its checksum differs from the recorded one.

This is exactly the semantics that lets a project keep its existing `migrations/` files without rewriting a single script.

## How to choose

| Situation | Recommendation |
|---|---|
| A historical database whose process cannot change | traditional mode |
| A new project with frequent clean initialisations in CI | Stage + Tail |
| Many DML migrations carrying data | Stage + Tail: `schema/` for structure, the tail for data |
| A CI check that "schema == checkpoint + tail" | Stage + Tail plus `stage verify` |

Next: [the traditional path](traditional.md) and [Stage + Tail](stage_tail.md).
