# Safe history pruning

Pruning is the replacement for squashing. Scripts the checkpoint has already absorbed can be deleted from `migrations/` without rewriting a single checksum and without moving `baseline`. The boundary is `checkpoint.toml`.

## The marker

```toml
[checkpoint]
pruned_up_to_version = "3.2"
schema_hash = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
```

- `pruned_up_to_version` — the version below which the history has already been folded;
- `schema_hash` — the SHA-256 of the catalog at that boundary.

`Checkpoint` is read through `toml` with `#[serde(deny_unknown_fields)]`, so a typo in a key or an extra section is an error rather than a silently ignored field. `Checkpoint::load` returns `Ok(None)` for a missing file and for a path that cannot exist (for example one below an ordinary file), but treats any other read error as an error: a silently ignored broken marker would let the tail run against a database that can no longer be advanced.

## The outdated-target guard

When `migrate up` receives `--checkpoint-path`, the runner is constructed through `Migrator::with_checkpoint`. Then, on every run:

1. `Checkpoint::validate_history` finds the highest successfully recorded version:
   - if a `<< Stage >>` entry with the boundary version is found, its checksum is additionally compared against `schema_hash` → on mismatch `Error::CheckpointHashMismatch { pruned_up_to_version, recorded_hash, expected_hash }`;
   - if the highest version is below the boundary (or there are no versions at all) → `Error::OutdatedCheckpoint { database_version, pruned_up_to_version, schema_hash }`, whose text directly suggests bringing the database up to the boundary first or rebuilding it from `schema/`.
2. Additionally, if the recorded version is *exactly* the boundary, `validate_catalog_hash` compares the live catalog (through `MigrationBackend::catalog_checksum`) against `schema_hash`. A database that drifted from its snapshot keeps the old digest in the history, so the live catalog is what gets checked; a backend that cannot introspect itself leaves the digest from the marker authoritative.

The "exactly at the boundary" check is a deliberate restriction: below the boundary the database is rejected as outdated, and above it the stage is already covered by the tail, so the checkpoint stops describing the schema.

## The command

```bash
cargo rurqe prune --before <version> [--migrations-dir <dir>] [--checkpoint-path <path>] [--dry-run]
```

What `prune_migrations` does:

1. Enters the `checkpoint.lock` section (`CheckpointLock::acquire`) — before reading the marker and before the first deletion. To every other writer of the artifacts the whole pruning looks like a single observation.
2. Reads the marker. No marker → an error: "`…` is missing; commit the stage with `cargo rurqe stage commit --version <before>` before pruning the history".
3. Checks the boundaries: if `checkpoint.pruned_up_to_version() < before` → an error: "anchors the history up to version X only; pruning below Y would drop scripts the active tail still needs".
4. Scans `migrations/` (`sql_scripts`) and parses every name through `Migration::parse(name, "")`: only scripts with a version `< before` are deleted; repeatable scripts are never deleted.
5. Deletes them (or, with `--dry-run`, prints "would prune") and finishes with the report "Pruned N migration(s) below version X".

A missing migrations directory is not an error but a polite "No migration directory … to prune".

### Why the deletions happen under the section

Deleting a script is the only mutation in this protocol that cannot be undone: neither an open nor a parked copy brings a deleted file back. Pruning therefore has no right to interleave with a commit (which has no live marker between its parking steps) or with a verification (which is precisely replaying the tail). Under the section, pruning either happens entirely before the commit and reads an already sealed marker, or entirely after it — and then the advanced marker excludes no fewer scripts.

| Interleaving | What the section removes |
|---|---|
| prune × commit | reading the marker inside the window where it is parked; rolling the boundary back under an already accepted decision |
| prune × verify | deleting scripts the verification is replaying at that moment |
| prune × prune | two deletions passing through each other's reads |

## Pruning checklist

1. Make sure every environment has passed the version below which you cut.
2. `cargo rurqe stage commit --version <boundary>` — fix the boundary (and the stage snapshot next to the marker).
3. `cargo rurqe stage verify` — make sure the equation holds.
4. `cargo rurqe prune --before <boundary> --dry-run` — inspect the list.
5. `cargo rurqe prune --before <boundary>` — delete.
6. Commit the file deletions together with `checkpoint.toml` and `<stem>-stage/`.

After that, `migrate up` on a database below the boundary fails with `OutdatedCheckpoint` — that is the protection working, not a regression.

Next: [The checkpoint protocol](checkpoint_protocol.md).
