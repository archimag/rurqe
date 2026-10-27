# Schema evolution: overview

Schema maintenance lives apart from the query code: `rurqe-core`, `rurqe-postgres` and `rurqe-sqlite` never import the migration machinery. The two layers meet at one artifact — the catalog snapshot (`schema.lock`) — not at a dependency.

Inside the lifecycle the problem splits in two:

- **the structure** of the database, described once in a declarative `schema/` directory;
- **the journey** of an already deployed database to that structure, described by a short tail in `migrations/`.

The second half on its own is the classic Flyway flow: versioned `V...` scripts, repeatable `R...` scripts, SHA-256 checksums, `repair` and `baseline`. It keeps working exactly as before — see [the traditional path](traditional.md).

## Three things that make the difference

### 1. Instant cold start for CI and tests

A clean database never replays history. It is built from the declaration:

```bash
cargo rurqe stage init --version 1.0
```

Every `schema/*.sql` is applied in one step and a single `<< Stage >>` marker lands in `__rurqe_schema_history`. The cost is O(current schema) instead of O(number of migrations), so a test container, a CI job or a new laptop is ready in milliseconds no matter how long the project has lived.

### 2. Pruning without squashing

Once every deployment has passed version `3.2`, the scripts below it are dead weight:

```bash
cargo rurqe prune --before 3.2 --dry-run   # see the list
cargo rurqe prune --before 3.2             # delete it
```

No checksum is rewritten, no baseline is re-cut, no history is squashed into one opaque script: the boundary lives in `checkpoint.toml`. A database still below that boundary is rejected with an actionable diagnostic instead of being advanced by a tail that no longer starts at its version.

### 3. An equality you can run in CI

```bash
cargo rurqe stage verify
```

materialises the declaration and the checkpoint-plus-tail in separate scratch databases and compares the catalogs:

```text
State(schema/)  ==  State(Checkpoint + migrations/)
```

The invariant is checked on every pull request instead of being assumed — see [The Golden Equation](golden_equation.md).

## A five-minute tour

```bash
# 1. The target schema is written once, as plain DDL
$EDITOR schema/users.sql

# 2. A clean database is created straight from it
cargo rurqe stage init --version 1.0 --database-url postgres://localhost/app

# 3. An existing deployment is advanced by the tail only
cargo db-migrate up

# 4. The current state is sealed as the new boundary
cargo rurqe stage commit --version 1.1

# 5. Nothing below the boundary is needed any more
cargo rurqe prune --before 1.1
```

## Where the pieces live

- `rurqe-migrate` — the script model, the history table, the runner, the declarative stage and the boundary marker. It is joined to the query crates by the catalog format, not by shared code.
- `rurqe-cli` — the commands (`migrate`, `baseline`, `codegen`, `verify`, `stage init|verify|commit|cleanup`, `prune`) and the artifacts they produce: `schema.lock`, `checkpoint.toml`, `<stem>-stage/`.

## Section contents

- [The crisis of traditional migrations in the AI era](problem.md) — why a linear script history stopped scaling.
- [Two modes and how to choose](modes.md) — what the modes share and where exactly they differ.
- [The traditional path: `V...` and `R...`](traditional.md) — naming, checksums, transactionality, `migrate`/`baseline`/`repair`.
- [Stage + Tail: declarative stage and cold start](stage_tail.md) — `schema/*.sql`, instant initialisation, the boundary commit.
- [The Golden Equation](golden_equation.md) — reconciling `State(schema/)` against `State(Checkpoint + tail)`.
- [Safe history pruning](pruning.md) — `prune --before` and the outdated-target guard.
- [The checkpoint protocol](checkpoint_protocol.md) — `checkpoint.toml`, `checkpoint.lock`, the owner record, recovery.

## The history table

Both models write into one `__rurqe_schema_history` table, so a project can switch between them without losing track. A cold start leaves a single `<< Stage >>` row of kind `Baseline`; the runner treats every version at or below it as already applied and reports those in `status` as `IgnoredBaseline`, while the tail above it keeps being applied by the ordinary `migrate up`.

## How to read this section

Every statement here is derived from the sources of `crates/rurqe-migrate` and `crates/rurqe-cli/src/{cli,stage}` and from their unit tests. `rurqe.toml` carries no key that enables a mode: `CodegenConfig::parse` rejects any unknown section and any unknown key, and it knows exactly `schema`, `[codegen]`, `[codegen].domain_types` and `[codegen].type_overrides`. The mode is determined by the set of directories and the set of commands, not by a configuration flag.
