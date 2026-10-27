# Safe history pruning

Pruning replaces squashing. Scripts the checkpoint already absorbed are deleted from `migrations/` without rewriting a single checksum and without moving `baseline`. The boundary is `checkpoint.toml`.

## The marker

```toml
[checkpoint]
pruned_up_to_version = "3.2"
schema_hash = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
```

- `pruned_up_to_version` — the version below which the history has already been folded;
- `schema_hash` — the SHA-256 of the catalog at that boundary.

The file is parsed by `toml` with `#[serde(deny_unknown_fields)]`, so a typo in a key or an extra section is an error rather than a silently ignored field. `Checkpoint::load` returns `Ok(None)` when the file is missing and when the path cannot exist (for example one below an ordinary file), but treats any other read failure as an error: a silently ignored broken marker would let the tail run against a database that can no longer be advanced.

## The checklist

1. Make sure every environment has passed the version below which you cut.
2. `cargo rurqe stage commit --version <boundary>` — fix the boundary and the stage snapshot next to the marker.
3. `cargo rurqe stage verify` — make sure the equation still holds.
4. `cargo rurqe prune --before <boundary> --dry-run` — inspect the list.
5. `cargo rurqe prune --before <boundary>` — delete.
6. Commit the file deletions together with `checkpoint.toml` and `<stem>-stage/`.

## The command

```bash
cargo rurqe prune --before <version> [--migrations-dir <dir>] [--checkpoint-path <path>] [--dry-run]
```

`prune_migrations` refuses to guess:

- **No marker** → an error naming the exact command to run first: "`…` is missing; commit the stage with `cargo rurqe stage commit --version <before>` before pruning the history".
- **`--before` beyond the marker** → an error: the marker anchors the history up to version `X` only, and pruning below a later version would drop scripts the active tail still needs.
- **Only versioned scripts below `--before` are removed.** Names are parsed with `Migration::parse`, and repeatable `R...` scripts are never deleted — they are meant to be re-applied.
- **`--dry-run` prints "would prune"** for each file and deletes nothing, and the run still ends with the count.
- **A missing migrations directory** is not an error, just "No migration directory … to prune".

The whole prune — the marker read, the `migrations/` scan and the file removals — runs inside the exclusive `checkpoint.lock` section the other artifact writers take. Deleting a script is the one mutation this protocol cannot undo, so a concurrent `stage commit` or `stage verify` must not interleave: either the pruning happens entirely before a commit and reads an already sealed marker, or entirely after it. A concurrent writer is reported with the pid of its live owner. The section and its recovery rules are described in [The checkpoint protocol](checkpoint_protocol.md).

## The outdated-target guard

Pruning is only safe because a database that lags behind the boundary is refused instead of being advanced by a partial tail. When `migrate up` receives `--checkpoint-path`, the runner is built through `Migrator::with_checkpoint`, and on every run:

1. `Checkpoint::validate_history` looks at the highest successfully recorded version:
   - if a `<< Stage >>` entry carries the boundary version, its checksum is compared against `schema_hash` → `Error::CheckpointHashMismatch { pruned_up_to_version, recorded_hash, expected_hash }`;
   - if the highest version is below the boundary (or no version was ever recorded) → `Error::OutdatedCheckpoint { database_version, pruned_up_to_version, schema_hash }`, whose text directly suggests bringing the database up to the boundary first or rebuilding it from `schema/`.
2. If the recorded version is exactly the boundary, `validate_catalog_hash` compares the live catalog against `schema_hash` as well. A backend that cannot introspect itself leaves the digest from the marker authoritative.

That "exactly at the boundary" restriction is deliberate: below the boundary the database is rejected as outdated, and above it the stage is already covered by the tail, so the checkpoint stops describing the schema.

`--checkpoint-path` guards `migrate up` alone: passing it to `migrate status` or `migrate repair` is rejected rather than silently accepted.

## After pruning

`migrate up` on a database below the boundary fails with `OutdatedCheckpoint` — that is the protection working, not a regression. Bring the database up to the boundary first, or rebuild an empty database from `schema/` with `stage init`.

Next: [The checkpoint protocol](checkpoint_protocol.md).
