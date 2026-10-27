# The Golden Equation

The claim `stage verify` checks is written as a single equality:

```text
State(schema/)  ==  State(Checkpoint + migrations/)
```

On the left is the declaration, on the right what actually results from taking the recorded boundary and replaying the active tail onto it. The check materialises both sides in separate scratch databases and compares the catalogs bit-for-bit.

## API

```rust
use rurqe_cli::stage::GoldenEquation;

let report = GoldenEquation::new(&stage, &tail)
    .with_schema("app")
    .with_base(base)
    .verify(scratch_url)?;
```

| Method | Meaning |
|---|---|
| `new(stage, tail)` | the equation with no base: both sides are replayed in full |
| `with_base(base)` | the replay starts from the boundary snapshot (`<stem>-stage/`) |
| `with_checkpoint(&checkpoint)` | versions `<= pruned_up_to_version` are skipped; the boundary hash is verified against the marker |
| `with_schema(name)` | the PostgreSQL schema both sides are materialised and introspected in |
| `verify(scratch_url)` | run it and return `GoldenReport` or `StageError` |

`GoldenReport` carries `stage_catalog`, `tail_catalog`, `diff` (`SchemaDiff`) and `schema_notices`. `is_equivalent()` is `diff.is_empty()`; `stage_hash()` and `tail_hash()` return the `schema.lock` hash of each side.

## Order of checks

1. **Empty boundary without a base.** If a checkpoint is given, no base is given, and `checkpoint.schema_hash()` is not the digest of the empty catalog, the equation is rejected with `StageError::BaseStageMissing`: otherwise the filtered tail would be replayed over an empty scratch database and a correct project would be declared diverged.
2. **The tail side.** The anchored side is materialised first, so the checkpoint hash is verified before the declarative stage is touched:
   - if a base is given, its scripts are applied; then, if a checkpoint is given, the boundary catalog is compared against `schema_hash` → on mismatch `StageError::BaseStageHashMismatch { expected, actual }` (the snapshot drifted from its own marker, and this is an error *before* any divergence report);
   - the tail is then applied in the same order as in the production runner (`Migrator::sort_migrations`), skipping versions `<= pruned_up_to_version`;
   - a script failure is wrapped into `StageError::Script { script, message }`.
3. **The stage side.** The declarative scripts are applied in declaration order.
4. **Comparison.** `compute_catalog_diff(&tail_catalog, &stage_catalog)`; an empty diff → `Ok(report)`, otherwise `Err(StageError::Diverged(Box<GoldenReport>))`.

The sandbox is discarded in both cases, even if the replay failed: half of an applied script must not be inherited by the next side. If the discard fails too, its diagnostic is folded into the replay diagnostic — the scratch stayed behind, and the next attempt will meet it as a foreign one.

## An empty selection is not a proof

The equation "nothing against nothing" formally holds. So that this does not turn into a false success, each materialised side interrogates its sandbox (`vacuous_schema_notice()`): if the selected PostgreSQL schema contains no relation while the database has user schemas with relations, `GoldenReport::schema_notices` receives a warning listing those schemas and the `--schema` hint. Identical warnings from both sides collapse into one.

## Rendering the findings

Divergences are printed in "stage versus database" wording for the commit (`DiffWording::COMMIT`) and in equation wording for the verification (`DiffWording::GOLDEN`). The format is the same — `SchemaDiff::write_details`.

## CLI

```bash
cargo rurqe stage verify [--stage-dir <dir>] [--base-stage-dir <dir>] \
                         [--migrations-dir <dir>] [--checkpoint-path <path>] \
                         [--scratch-url <url>] [--schema <name>]
```

The command proceeds as follows:

1. takes the `checkpoint.lock` section (§ [The checkpoint protocol](checkpoint_protocol.md));
2. recovers an interrupted commit — safe under the section, while a live writer simply refuses the acquisition;
3. reads the stage, the tail and the marker;
4. loads the base: an explicit `--base-stage-dir`, or the `<stem>-stage/` snapshot next to the marker. If there is no snapshot the base stays undefined — and the equation itself rejects a checkpoint it cannot reproduce from nothing;
5. releases the section: only scratch-database work remains;
6. runs the equation and prints either "Golden equation holds: State(...) == State(Checkpoint + ...)" or the divergences.

If the recovery already renamed the artifacts and a subsequent read or the equation itself failed, the recovery report is attached to the error (`carry_recovery`): the error channel is the only output the command guarantees.

See also: [Safe history pruning](pruning.md).
