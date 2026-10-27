# The Golden Equation

`stage verify` proves one equality and fails the build when it stops holding:

```text
State(schema/)  ==  State(Checkpoint + migrations/)
```

On the left is the declaration. On the right is what actually results from taking the recorded boundary and replaying the active tail onto it. Both sides are materialised in separate scratch databases and their catalogs are compared bit-for-bit.

## Why run it

- Editing `schema/` and forgetting the migration — or editing the migration and forgetting `schema/` — turns a silent production surprise into a red CI check.
- The boundary snapshot is proven to reproduce the catalog `checkpoint.toml` claims, so a historical commit cannot quietly stop being replayable.
- The tail is proven to be complete: a database created from the declaration and one upgraded through the tail end up with the same schema.

## Running it

```bash
cargo rurqe stage verify [--stage-dir <dir>] [--base-stage-dir <dir>] \
                         [--migrations-dir <dir>] [--checkpoint-path <path>] \
                         [--scratch-url <url>] [--database-url <url>] [--schema <name>]
```

The command proceeds as follows:

1. takes the `checkpoint.lock` section (§ [The checkpoint protocol](checkpoint_protocol.md));
2. recovers an interrupted commit — safe under the section, while a live writer simply refuses the acquisition;
3. reads the stage, the tail and the marker;
4. loads the base: an explicit `--base-stage-dir`, or the `<stem>-stage/` snapshot next to the marker. Without a snapshot the base stays undefined, and the equation itself rejects a checkpoint it cannot reproduce from nothing;
5. releases the section — only scratch-database work remains;
6. prints either `Golden equation holds: State(...) == State(Checkpoint + ...)` or the divergences.

If the recovery already renamed the artifacts and a later read or the equation itself failed, the recovery report is attached to the error: the error channel is the only output the command guarantees.

## What happens on each side

The anchored side runs first, so the boundary hash is verified before the declaration is touched:

- the base scripts are applied;
- the boundary catalog is compared against `checkpoint.schema_hash`; a mismatch fails with `StageError::BaseStageHashMismatch { expected, actual }` — the snapshot drifted from its own marker, which is an error before any divergence report;
- the tail is then applied in the production order (`Migrator::sort_migrations`), skipping versions `<= pruned_up_to_version`;
- a failing script is reported as `StageError::Script { script, message }`.

Then the declarative side applies `schema/*.sql` in declaration order. Finally `compute_catalog_diff(&tail_catalog, &stage_catalog)` runs: an empty diff succeeds, anything else fails with `StageError::Diverged(Box<GoldenReport>)`.

An empty boundary given without a base is rejected up front with `StageError::BaseStageMissing`: replaying a filtered tail over an empty scratch database would declare a correct project diverged.

The sandbox is discarded on both outcomes, including a failed replay — half an applied script must not be inherited by the next side. A failing discard is folded into the replay diagnostic, so a scratch database that stayed behind is named instead of being met as a foreign one next time.

## Reading the output

- `Golden equation holds: ...` — the proof.
- A divergence list — the same report format the commit parity gate prints, worded as an equation rather than as "stage versus database". It names the object and the difference, so the fix is either a missing migration or a stale `schema/`.
- Scratch databases left behind by a crash — `cargo rurqe stage cleanup` lists them, `--yes` drops them.

## An empty selection is not a proof

The equation "nothing against nothing" formally holds. So that this does not turn into a false success, each materialised side interrogates its sandbox: if the selected PostgreSQL schema contains no relation while the database holds user schemas that do, the report carries a warning listing those schemas and pointing at `--schema`. Identical warnings from both sides collapse into one.

## In CI

```bash
cargo rurqe stage verify --database-url "$DATABASE_URL" --scratch-url "$SCRATCH_URL"
```

PostgreSQL verifications materialise into disposable scratch databases; SQLite ones work in memory, so the check needs no infrastructure at all.

## As a library

The same check is available without the CLI:

```rust
use rurqe_cli::stage::GoldenEquation;

let report = GoldenEquation::new(&stage, &tail)
    .with_schema("app")
    .with_base(base)
    .verify(scratch_url)?;
```

`GoldenReport::is_equivalent()` is the verdict, `stage_hash()` and `tail_hash()` expose the `schema.lock` hash of each side, and an empty `diff` is what makes the report equivalent.

See also: [Safe history pruning](pruning.md).
