# The checkpoint protocol

The pair "marker `checkpoint.toml` + boundary snapshot" describes one state. Replacing it must be atomic for a reader and safe when the process is killed at any moment, and concurrent writers (`stage commit`, `stage verify`, `prune`) must not interfere with each other. One module answers these requirements — `crates/rurqe-cli/src/cli/commands/stage/{lock,swap}.rs`.

## Artifacts and states

At any moment every artifact is in exactly one of the states: **live** (at its usual path), **staged** (assembled alongside, invisible under the live path), **parked** (renamed aside and the only recoverable copy of the previous state), **absent**.

Names are derived from the marker stem (`marker_stem`), so two markers in one directory never share a lock or a boundary:

| Artifact | live | staged | parked |
|---|---|---|---|
| marker | `<dir>/checkpoint.toml` | `<dir>/.checkpoint.toml.<pid>.<seq>.tmp` | `<dir>/.checkpoint.toml.<pid>.<nonce>.previous` |
| boundary | `<dir>/checkpoint-stage` | `<dir>/.checkpoint-stage.<pid>.<seq>.tmp` | `<dir>/.checkpoint-stage.<pid>.<nonce>.previous` |
| catalog snapshot | `<dir>/schema.lock` | through the same staged mechanism | never parked |
| section | `<dir>/checkpoint.lock` | not staged | never parked or deleted |
| owner record | `<dir>/checkpoint.lock.record` | published by an atomic rename | deleted on release |

`<pid>.<nonce>` is the writer's area: the pid separates live processes, the nonce prevents a reused pid from inheriting the parking of a previous one.

## `checkpoint.lock`: the single-writer section

- The file is opened `read+write+create`, without `truncate`, and a non-blocking exclusive advisory lock is taken through `fs4` (`flock` on Unix, `LockFileEx` on Windows).
- Nobody ever writes into the file itself: the kernel arbitrates the section by the identity of the file. The file is never deleted — otherwise the next writer would take a lock on a fresh file while the previous holder still owns the old one.
- The owner is published into the neighbouring `checkpoint.lock.record` after the section has been acquired: `pid`, `nonce`, `heartbeat` (nanoseconds at the moment the section was entered). The body is staged into a temporary neighbour and renamed as a whole, so a concurrent reader sees either a complete previous record or a complete new one — never a truncated one. The rename here is atomic, because the record file carries no advisory lock.
- Liveness is determined by the kernel, not by the record: an advisory lock belongs to an open file description and is released the moment it is closed, which the OS does for any terminated process. Therefore a hang, a long network round-trip or a long scratch can never declare a live holder dead — the heartbeat is not needed for that.
- The acquisition waits `ACQUIRE_WAIT` (10 s in production, 200 ms in tests), polling every `ACQUIRE_POLL` (100 ms), and then refuses, naming the owner from the record: "the checkpoint lock `…` is held by pid N (nonce M), in its section for K second(s); …". There is no preemption and there cannot be one.
- `Drop` deletes the published record (while the section is still held), then the descriptor is closed and the kernel releases the lock. A released writer leaves no identity behind for a reused pid to inherit.

## Commit: the sequence

Preparation happens outside the section (loading the stage, introspecting the live database, the parity scratch, rendering the lock and the marker, assembling the staged boundary) — it mutates no artifact of the working copy and may take as long as needed.

| Step | Where | Action | State afterwards |
|---|---|---|---|
| 0 | outside the section | assemble and flush the staged boundary | live(prev)/live(prev), staged(new) |
| 1 | in the section | park the previous marker | parked(prev)/live(prev), staged(new) |
| 2 | in the section | park the previous boundary | parked(prev)/parked(prev), staged(new) |
| 3 | in the section | rename the staged boundary into place and flush the directory | parked(prev)/live(new) |
| 4 | in the section | write the new marker and flush the directory — the commit point | live(new)/live(new) |
| 5 | in the section | delete the parked pair | live(new)/live(new), no recoverable copy |

Between steps 0 and 1 (already inside the section) the following happen: sweeping the staged garbage of previous attempts, recovery of an interrupted commit, the no-version-rollback rule, and writing `schema.lock`.

The order of steps 1 and 2 is what makes the sequence recoverable: the marker is parked first, so the boundary it pointed at stays live, and a kill between the two parkings only requires putting the marker back. The reverse order would leave a live marker describing a missing boundary.

Any failure inside the section before step 4 is rolled back: what was parked is returned, the staged directory is deleted (`StagedCleanup`), the command fails without touching the previous pair. From step 4 the commit has happened, so an undeleted parked pair is a warning, not a rollback.

## Recovering an interrupted commit

`recover_interrupted_commit` works under the section, so it only sees a pair left behind by a dead writer. The entry in `checkpoint.lock.record` must match the caller's area — otherwise the pair belongs to another writer (including a reused pid) and the recovery refuses, naming the recorded owner.

| Observation | Verdict | Action |
|---|---|---|
| no parked names | nothing was interrupted | nothing |
| marker live and boundary live | the commit reached its commit point | delete all parked copies, flush the directory |
| boundary live, its marker parked | the boundary was installed without the commit point | delete the installed boundary, return the parked one |
| marker live, its boundary parked | the marker was installed without the boundary | delete the installed marker, return the parked one |
| both parked | the commit stopped between the parkings | return the boundary first, then the marker |
| the record names another writer | this is not the caller's garbage | refuse with a diagnostic |

If the recovery has already renamed the artifacts and a subsequent step failed, the recovery report is attached to the error (`carry_recovery`) with an explicit note that the working copy has already changed.

## Invariants

- **I1 (marker ↔ boundary).** A live marker always describes the boundary next to it. The marker is written only after the boundary is installed and returned only after the boundary is returned.
- **I2 (one recoverable copy).** At most one copy of the previous pair exists at any moment; it belongs to the writer that parked it and is deleted only after that writer's commit point has become durable.
- **I3 (writer isolation).** Nobody renames, returns or deletes a parked copy of another live writer.
- **I4 (one writer).** At most one writer mutates the artifacts at a time; a second one either waits for the section or fails with a diagnostic naming the owner.
- **I5 (lock ownership).** Only its holder releases the section, the kernel does it on its death, and nobody deletes a record belonging to another.
- **I6 (no read across a swap).** Every read of the pair happens inside the section, so an observation never spans a swap.

## What the tests pin down

The regression suite is paired: the negative half checks the diagnostics and the absence of damage to foreign artifacts, the positive half checks that consecutive operations converge to a consistent state and that recovery after a crashed holder returns the pair and leaves the section free. Covered: two interleaved commits, an injected failure at every step of `seal_commit`, a verification during an in-flight commit, a reused pid with a stale record, two markers in one directory (separate sections, separate boundaries, no interference with a foreign parked pair) and the grammar of staged names, including the case of a foreign host with the same pid. Contention is exercised between two descriptors of one process, so a second process is not needed.

See also: [Safe history pruning](pruning.md) and [The Golden Equation](golden_equation.md).
