//! Crash-safe installation of the checkpoint marker and the boundary snapshot.
//!
//! A commit replaces two artifacts that together describe one state: the marker
//! `checkpoint.toml` and the verbatim boundary snapshot of the declarative stage.
//! The previous pair is parked before the new boundary is published, the marker is
//! written last as the commit point, and only then is the parked pair discarded,
//! so a process killed in between leaves a state [`recover_interrupted_commit`]
//! restores instead of a fresh boundary described by a stale marker.
//!
//! Being kill-safe is not the same as being multi-writer-safe: `stage commit`,
//! `stage verify`, and `prune` all mutate the same workspace artifacts, and two of
//! them running at once can discard the only copy the other one could still
//! restore. The protocol therefore adds one exclusive `checkpoint.lock` that
//! serialises every writer, gives each parked copy a writer scope so no writer can
//! ever rename, restore, or discard a copy another writer parked, and recovers a
//! writer that died holding the lock from the kernel's release of its section instead
//! of from a guess.
//!
//! # Protocol record
//!
//! The sections below are the normative contract of this module tree: the states an
//! artifact can be in, the critical section each command holds, the transitions a
//! commit walks through, the recovery classification, the interleavings of two
//! writers, and the invariants every action preserves. The record is written ahead
//! of the installer that satisfies it, so the code beside it is expected to match
//! the record rather than the record matching the code; a change to the protocol
//! edits this record first, together with the regression pairs it names.
//!
//! ## Artifact states
//!
//! At any instant every artifact below is in exactly one of four states:
//!
//! | State | Meaning |
//! | --- | --- |
//! | live | the entry sits at the path every command reads it from |
//! | staged | the new bytes are assembled beside the artifact and are not visible under the live path |
//! | parked | the previous copy was renamed aside and is the one recoverable copy of the state before the commit |
//! | absent | no entry at the path |
//!
//! The writer scope of a name is `<pid>.<nonce>`: the process id keeps two live
//! processes apart, and the nonce keeps a recycled id from inheriting the parking of
//! an earlier one. A commit takes that scope from the owner record the section
//! installed when it was acquired, never from a later re-read of the record, so a
//! record another writer rewrote later cannot make the writer park under an
//! identity that is not its own, so every parked copy stays attributable to the
//! scope that created it.
//!
//! * marker — live `<dir>/checkpoint.toml`, staged
//!   `<dir>/.checkpoint.toml.<pid>.<seq>.tmp`, parked
//!   `<dir>/.checkpoint.toml.<pid>.<nonce>.previous`.
//! * boundary — live `<dir>/<stem>-stage`, staged
//!   `<dir>/.<stem>-stage.<pid>.<seq>.tmp`, parked
//!   `<dir>/.<stem>-stage.<pid>.<nonce>.previous`.
//!
//! `<stem>` is the file stem of the marker, so the default `checkpoint.toml` keeps
//! the `checkpoint-stage` directory it has always described, while any other marker
//! describes a boundary of its own — `prod.toml` is guarded by `prod.lock` and
//! snapshotted into `prod-stage`. The stem is derived once, by the single function
//! both names are built from, so two markers in one directory never share a lock or
//! a boundary: the isolation follows from the naming scheme instead of from a
//! convention about one marker per directory.
//! Two further artifacts take part in the same critical section without taking part
//! in the swap:
//!
//! * `schema.lock` is written through the staged-and-renamed discipline but is never
//!   parked. It describes the live catalog the commit read, and the same commit
//!   recomputes it, so there is no previous copy worth restoring.
//! * `checkpoint.lock` is opened read+write+create and taken with a non-blocking
//!   exclusive advisory lock, so the kernel arbitrates the acquisition atomically and
//!   the entry is never staged. It is never parked and never removed either, since it
//!   describes the section and not a state a reader restores: an unlinked entry would
//!   let the next writer lock a fresh file while the previous holder still owns the old
//!   one. The entry itself carries no content: the owner record of the holder lives
//!   in a `checkpoint.lock.record` sibling, published once the section is held and
//!   removed when the guard drops. Its states are free (no description holds the
//!   advisory lock) and held (the kernel holds the section for one description, whose
//!   owner record names the pid and the nonce).
//!
//! ## Critical sections
//!
//! * `stage commit` holds the section across the sweep of staging debris, the
//!   recovery pass, the rewind guard, the `schema.lock` publication, and
//!   `seal_commit`. The stage load, the live-catalog introspection, the parity
//!   scratch materialisation, the rendering of the lock and the marker, and the
//!   staging of the boundary snapshot are preparation outside the section: none of
//!   them mutates a workspace artifact, so one writer's preparation may overlap
//!   another writer's section, and the rewind guard and the recovery inside the
//!   section re-observe whatever the intervening writer committed.
//! * `stage verify` holds it across the recovery pass and the reads of the marker,
//!   the base boundary, and the tail. The scratch databases stay outside it: each is
//!   per-verification and the catalog work on it is serially safe on its own.
//! * `prune` holds it across the checkpoint read, the `migrations/` scan, and the
//!   deletions.
//!
//! Command signatures do not change: the dispatcher reaches the same entry points,
//! and each of them takes the section itself, so no caller can forget it.
//!
//! ## Commit transitions
//!
//! A commit walks the sequence below; the right column is the resulting state of the
//! marker/boundary pair. Step 0 is preparation and runs outside the section, while
//! every later step runs inside it and is bounded by local filesystem work alone.
//!
//! | Step | Section | Action | State after the step |
//! | --- | --- | --- | --- |
//! | 0 | outside | the new boundary is assembled into the staging directory and flushed | live(prev) / live(prev), staged(new) |
//! | 1 | inside | the previous marker is parked | parked(prev) / live(prev), staged(new) |
//! | 2 | inside | the previous boundary is parked | parked(prev) / parked(prev), staged(new) |
//! | 3 | inside | the staged boundary is renamed into place and the directory flushed | parked(prev) / live(new) |
//! | 4 | inside | the new marker is written and the directory flushed — the commit point | live(new) / live(new) |
//! | 5 | inside | the parked pair is discarded | live(new) / live(new), no recoverable copy left |
//!
//! The section itself is taken, the debris swept, the interrupted pair repaired, and
//! `schema.lock` written between step 0 and step 1.
//!
//! Every failure inside the section before step 4 is rolled back: whatever was
//! already parked is renamed back and the staged directory is discarded, so the
//! command reports a failure with the previous pair untouched. A failure between step
//! 0 and the section discards the staged directory alone. From step 4 on the commit
//! has happened, so a parked pair that cannot be removed is reported as a warning and
//! swept by a later writer, never rolled back.
//!
//! The order of steps 1 and 2 is what makes the sequence recoverable: parking the
//! marker first leaves the boundary the parked marker describes still live, so a
//! kill between the two parks needs only the marker put back, while parking the
//! boundary first would leave a live marker describing a boundary that is gone.
//!
//! ## Recovery classification
//!
//! `recover_interrupted_commit` runs under the section, so it observes only a pair a
//! writer left behind when it died — never a pair a live writer is between two steps
//! of. The record has to echo the scope its caller reports before a copy is classified:
//! a record naming any other writer — a stale record whose pid recycled into the caller
//! included — marks a repair running outside the section that owns the pair and is
//! refused. A caller whose record echoes classifies what it finds and takes exactly one
//! action:
//!
//! | Observation | Verdict | Action | Invariant |
//! | --- | --- | --- | --- |
//! | no parked name found | nothing was interrupted | nothing | I1, I2 |
//! | marker live and boundary live | the commit reached its commit point | discard every parked copy, flush the directory | I1, I2 |
//! | boundary live, its marker parked | a boundary installed without its commit point | drop the installed boundary, restore the parked one | I1 |
//! | marker live, its boundary parked | a marker installed without its boundary | drop the installed marker, restore the parked one | I1 |
//! | both parked | the commit stopped between the two parks | restore the boundary, then the marker | I1, I2 |
//! | the record names a writer other than the caller's scope | not this caller's debris and not recoverable here | refuse with a diagnostic naming the recorded writer and the reported scope | I3, I5 |
//!
//! The last row is the rule the writer scopes carry: a parked copy belongs to the
//! writer whose scope its name carries, and a caller whose record does not echo its own
//! scope — a stale record whose pid recycled into it included — repairs nothing, so no
//! writer touches another writer's copy even when the protocol went wrong around it.
//!
//! ## Interleavings of two writers
//!
//! ### commit × commit
//!
//! Two commits over fixed `.previous` names break the pair in two ways. A second
//! writer that enters recovery while the first is between steps 3 and 4 sees a live
//! marker beside a live boundary, concludes that the pair reached its commit point,
//! and discards the parked pair the first writer still needs — invariant I2 broken. A
//! second writer that starts while the first is between steps 2 and 4 renames the
//! previous boundary back over the boundary the first writer just installed, so the
//! marker the first writer goes on to write describes a boundary it no longer sits
//! beside — invariants I1 and I2 broken at once.
//!
//! Under the section both are serialised: the second writer waits for it, takes it
//! once the first releases it, and then finds either a completed commit, whose parked
//! pair is debris, or a pair whose owner is gone, which it repairs. While the first
//! writer is alive the second one never reaches the artifacts at all: its bounded wait
//! expires and it fails with a diagnostic naming the live owner. That refusal is the
//! negative half of the commit × commit regression pair; the positive half runs the
//! two commits one after another and asserts that the pair converges to the second
//! commit with at most one recoverable copy in between.
//!
//! ### commit × verify
//!
//! A verification mutates the artifacts too: it repairs an interrupted commit before it
//! reads anything, and then reads the marker, the base boundary, and the tail. Unlocked,
//! its repair can restore a parked boundary underneath a commit that is between steps 3
//! and 4, so the commit publishes a marker describing a boundary another writer
//! replaced. Its reads are no safer: between steps 2 and 4 the boundary is absent under
//! its live path, so a verification would replay the tail over no base at all and reject
//! a project whose commit is perfectly healthy. Holding the section across the repair
//! and the reads means a verification that overlaps a commit waits, and one that runs
//! after it reads exactly the committed pair, never a half-swapped one.
//!
//! ### commit × prune
//!
//! Between steps 1 and 4 the marker does not exist under its live path: it sits parked,
//! waiting for the commit point. A prune that reads the marker in that window sees a
//! project without a checkpoint and refuses, so a healthy project reports the "commit the
//! stage before pruning" failure; a prune that repairs the pair first instead decides from
//! the boundary the in-flight commit is about to replace. The deletions themselves are the
//! one mutation this protocol cannot undo — neither a parked nor a staged copy brings a
//! removed script back — so they must also never overlap a verification replaying the tail.
//! Inside the section a prune either runs entirely before a commit, where it reads the
//! committed marker and removes only what that marker already excludes, or entirely after
//! it, where the advanced marker excludes at least as much.
//!
//! | Interleaving | Failure the section removes | Invariants |
//! | --- | --- | --- |
//! | commit × commit | discarding the only recoverable copy; restoring over a fresh boundary | I1, I2, I3, I4 |
//! | commit × verify | a repair underneath a swap; a comparison of a half-swapped pair | I1, I4, I6 |
//! | commit × prune | a marker read in its parked window; deletions racing a tail replay | I2, I4, I6 |
//!
//! ## The lock itself
//!
//! `checkpoint.lock` is a sibling of the marker, opened read+write+create instead
//! of create-new, because the entry is never removed and never written into, and
//! taken with a non-blocking exclusive advisory lock (`flock` on Unix,
//! `LockFileEx` on Windows) through the `fs4` crate. The entry carries no content,
//! because the kernel arbitrates the section on the identity of the file alone;
//! the owner record of the writer (pid, nonce, and the instant it entered the
//! section) lives in a `checkpoint.lock.record` sibling, published once the
//! section is held and removed when the guard drops. The record publishes
//! atomically — its body staged in a hidden sibling and renamed over the record
//! file only once complete, sound because nothing holds an advisory lock on the
//! record file — so a concurrent reader meets either the whole previous record or
//! the whole fresh one, never a torn body. The record names that owner in a
//! refusal and scopes the copies a commit parks. Acquisition polls the lock for a
//! bounded time and then fails naming that owner — or stating that the holder
//! cannot be identified, when no record can be read from it; it never takes a
//! section over, because it cannot: the kernel releases the advisory lock when
//! its holder's file description closes, which happens for every process that
//! exits however it dies, so a section held by a live writer is never preempted
//! and a section held by a crashed writer is simply free. Release is RAII:
//! dropping the guard removes the record it published and closes the description,
//! so the kernel releases the section, and a released writer leaves no identity
//! behind — a recycled pid inherits none — while no writer ever removes an entry
//! another writer owns. Contention is observable between two descriptions of one
//! process, so the refusal and the recovery are testable without a second
//! process. The lock adds one dependency, and is panic-free.
//!
//! ## Invariants
//!
//! * I1 (marker ↔ boundary): a live marker always describes the boundary beside it. The
//!   marker is written only after its boundary is in place and restored only after the
//!   boundary it describes has been restored.
//! * I2 (one recoverable copy): at most one recoverable copy of the previous pair exists
//!   at any instant, it is owned by the writer that parked it, and it is discarded only
//!   after that writer's commit point is durable — or after its owner is provably dead.
//! * I3 (writer isolation): no writer ever renames, restores, or removes a parked copy
//!   another live writer created.
//! * I4 (one writer): at most one writer mutates the workspace artifacts at a time, and a
//!   second one either waits for the section or fails with a diagnostic naming the
//!   current owner.
//! * I5 (lock ownership): the section is released only by its holder, and the kernel
//!   releases it when that holder dies, so no writer holds a section another live writer
//!   still owns and no writer ever removes an entry another writer owns.
//! * I6 (no half-swapped read): every read of the pair happens inside the section, so no
//!   observation spans a swap.
//!
//! ## What the regression suite pins
//!
//! Each test is paired: the negative pin asserts the diagnostic and the absence of damage
//! to another writer's artifacts, the positive pin asserts that sequential operations
//! converge to a consistent state and that a crashed-holder recovery repairs the pair and
//! leaves the entry re-acquirable by the next writer. The pairs are two interleaved
//! commits, an injected failure at every `seal_commit` step (with the previous boundary
//! still recoverable at each one of them), a verification concurrent with an in-flight
//! commit, a verification concurrent with a live writer that is between the steps of its
//! swap — where the negative pin refuses with that writer's pid and nonce named and leaves
//! the pair it parked untouched (I1–I3), and the positive pin converges once the holder is
//! gone, discarding the parked pair as the debris of a writer whose death the kernel proved
//! (I2) — a recycled pid whose stale record does not echo the caller's scope, where the
//! negative pin refuses the repair and leaves the parked pair exactly where its dead
//! writer left it, and the positive pin converges once the section is taken and the
//! record rewritten with the caller's own scope — and a prune concurrent with a reader.
//! Two markers in one directory pin the artifact identity itself: the disjoint sections
//! (`two_markers_in_one_directory_hold_their_sections_at_once`), the disjoint boundaries
//! (`two_markers_commit_independently_in_one_directory`), and the non-interference of a full
//! cycle over one marker with the parked pair and the staging debris of the other
//! (`a_cycle_over_one_marker_leaves_the_parked_pair_of_another_untouched`,
//! `a_sweep_of_one_marker_keeps_the_staged_debris_of_another`), whose pair
//! `the_parked_pair_of_another_marker_is_repaired_under_its_own_section` then restores under
//! the section it belongs to. A cross-host pid collision is pinned by the staging gate —
//! `a_staged_path_of_another_host_sharing_this_pid_is_never_this_processes`,
//! `a_staged_path_of_this_very_scope_is_debris_of_an_earlier_attempt`,
//! `a_staged_path_of_a_live_foreign_process_is_never_claimed`, and
//! `a_staged_path_of_a_provably_dead_process_is_claimable` — together with the name grammar
//! (`copies_of_one_pid_under_different_nonces_never_share_a_name`), the refusal naming the
//! owner of an occupied staging path (`a_staging_path_a_live_writer_holds_is_refused_by_name`),
//! and the discard that tolerates an absent path
//! (`the_debris_of_a_dead_writer_is_discarded_and_an_absent_path_is_no_error`). They run over
//! the real filesystem in a temporary directory, while the contention they lean on is
//! observable between two descriptions of one process and needs no second one.

mod park;
mod recover;
mod seal;

pub(super) use self::recover::path_exists;
pub(crate) use self::recover::recover_interrupted_commit;
pub(crate) use self::seal::write_staged_boundary;
pub(super) use self::seal::{boundary_dir, create_staging_dir, seal_commit};
#[cfg(test)]
pub(crate) use self::seal::{flush_boundary_parent, move_failure};
