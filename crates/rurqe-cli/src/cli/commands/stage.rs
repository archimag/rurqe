//! Declarative stage, checkpoint, and scratch lifecycle commands.
//!
//! The module carries the command layer of the lifecycle — the cold start, the
//! golden equation, the commit that seals a stage, and the scratch sweep — while
//! the crash-safe installation of the marker and the boundary it publishes lives
//! in [`swap`], so the artifact protocol stays auditable in one place.

use std::fmt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use rurqe_migrate::checkpoint::Checkpoint;
use rurqe_migrate::runner::MigrationBackend;
use rurqe_migrate::stage::{cold_start, DeclarativeStage, StageMarker};
use rurqe_migrate::version::MigrationVersion;

use crate::cli::{connect, redact_database_url, CliError, CommandOutput, Connection};
use crate::drift::{DiffWording, SchemaDiff};
use crate::introspect::introspect_database_schema_notice;
use crate::loader::load_migrations_from_dir;
use crate::stage::postgres::{
    current_owner_tag, drop_stale_scratch_databases, find_stale_scratch_databases,
    ScratchAdminSession,
};
use crate::stage::{compare_stage_with_live, GoldenEquation};
use crate::target::{DatabaseDialect, DatabaseTarget};

use super::{
    cli_error, sweep_stale_temporaries, sync_directory, write_file, STALE_TEMPORARY_AGE,
};

pub(super) mod lock;
mod swap;

pub(crate) use self::swap::recover_interrupted_commit;

// The staged boundary snapshot is assembled by the preparation phase of a commit,
// which lives in this module, so the writer is published to the module tree rather
// than to the crate; the unit tests of the tree reach it the same way.
pub(super) use self::swap::write_staged_boundary;

// The rollback helpers are driven directly by the unit tests of this module tree,
// so the entry points they exercise are re-exported for them alone: a build
// without tests reaches the protocol through the commit that owns it.
#[cfg(test)]
pub(crate) use self::swap::{flush_boundary_parent, move_failure};

use self::lock::CheckpointLock;
use self::swap::{boundary_dir, create_staging_dir, path_exists, seal_commit};

/// Drops the PostgreSQL scratch databases earlier verifications abandoned.
///
/// Requires explicit confirmation via `force` (`--yes` / `--force`) before dropping.
/// The candidates are read from the catalog exactly once, so the list the
/// confirmation names is the very list `--yes` drops. Without confirmation, the
/// candidates are named in the reported failure so the operator sees exactly
/// what the confirmation would drop.
pub(crate) fn cleanup_stage(
    scratch_url: &str,
    older_than: Duration,
    force: bool,
) -> Result<CommandOutput, CliError> {
    if DatabaseTarget::parse(scratch_url).dialect() != DatabaseDialect::Postgres {
        return Ok(CommandOutput::single(format!(
            "Nothing to sweep: `{}` addresses a SQLite scratch database, which a verification \
             opens in memory or in the file it is pointed at",
            redact_database_url(scratch_url)
        )));
    }

    let mut admin = match connect(scratch_url)? {
        Connection::Postgres(client) => client,
        Connection::Sqlite(_) => {
            return Err(CliError(format!(
                "`{}` does not address a PostgreSQL database",
                redact_database_url(scratch_url)
            )))
        }
    };

    cleanup_scratch_catalog(&mut admin, older_than, force, scratch_url)
}

/// Sweeps the stale scratch databases of the catalog `session` administers.
///
/// The catalog is scanned exactly once: the candidate list the confirmation
/// names is the very list `--yes` drops, so a catalog that changed between two
/// reads can no longer make the sweep drop something the operator never
/// confirmed.
pub(super) fn cleanup_scratch_catalog<S: ScratchAdminSession + ?Sized>(
    session: &mut S,
    older_than: Duration,
    force: bool,
    scratch_url: &str,
) -> Result<CommandOutput, CliError> {
    let owner_tag = current_owner_tag();
    let candidates =
        find_stale_scratch_databases(session, older_than, &owner_tag).map_err(cli_error)?;

    if candidates.is_empty() {
        return Ok(CommandOutput::single(format!(
            "No PostgreSQL scratch database on `{}` is older than {} second(s)",
            redact_database_url(scratch_url),
            older_than.as_secs()
        )));
    }

    if !force {
        let mut message = format!(
            "`stage cleanup` requires `--yes` (or `--force`) to confirm dropping stale scratch \
             databases; found {} stale database(s) on `{}` older than {} second(s):",
            candidates.len(),
            redact_database_url(scratch_url),
            older_than.as_secs()
        );
        for database in &candidates {
            message.push_str(&format!("\n  - {database}"));
        }
        return Err(CliError(message));
    }

    let dropped = drop_stale_scratch_databases(session, &candidates, scratch_url);

    if dropped.is_empty() {
        return Ok(CommandOutput::single(format!(
            "No PostgreSQL scratch database on `{}` could be dropped",
            redact_database_url(scratch_url)
        )));
    }

    let mut output = CommandOutput::new();
    output.line(format!(
        "Dropped {} stale PostgreSQL scratch database(s) on `{}` older than {} second(s):",
        dropped.len(),
        redact_database_url(scratch_url),
        older_than.as_secs()
    ));
    for database in &dropped {
        output.line(format!("  - {database}"));
    }

    Ok(output)
}

/// Loads the declarative stage from `dir`, rejecting an absent directory.
fn load_stage(dir: &Path) -> Result<DeclarativeStage, CliError> {
    DeclarativeStage::load(dir)
        .map_err(cli_error)?
        .ok_or_else(|| {
            CliError(format!(
                "declarative stage `{}` does not exist; it holds the `*.sql` DDL of the target \
                 schema",
                dir.display()
            ))
        })
}

/// Initialises a cold database from the declarative stage.
///
/// The recorded [`StageMarker`] anchors the canonical catalog digest — the same
/// digest `schema.lock` and `checkpoint.toml` carry — instead of the raw DDL
/// digest of the stage directory. The backend derives that digest from the
/// catalog the stage produced under the engine that really executes it, so a
/// PostgreSQL stage is never measured through SQLite; only a backend that cannot
/// introspect itself keeps the stage digest supplied here as its fallback. A
/// PostgreSQL backend reads the schema selected for the command, so the marker
/// anchors the very relations the project declares.
pub(crate) fn initialise_stage<B: MigrationBackend>(
    backend: &mut B,
    stage_dir: &Path,
    version: MigrationVersion,
    description: &str,
) -> Result<CommandOutput, CliError> {
    let stage = load_stage(stage_dir)?;

    if stage.is_empty() {
        return Err(CliError(format!(
            "declarative stage `{}` holds no `*.sql` script to apply",
            stage_dir.display()
        )));
    }

    let marker = StageMarker::new(version, description, stage.checksum());
    let report = cold_start(backend, &stage, &marker).map_err(cli_error)?;

    if report.stage_scripts.is_empty() {
        return Ok(CommandOutput::single(format!(
            "Database already initialised at stage {}; nothing to apply",
            marker.version()
        )));
    }

    let mut output = CommandOutput::new();
    output.line(format!(
        "Initialised the database from {} script(s) at stage {} in {} ms",
        report.stage_scripts.len(),
        marker.version(),
        report.total_time_ms
    ));
    for script in &report.stage_scripts {
        output.line(format!("  - {script}"));
    }
    Ok(output)
}

/// Verifies the golden equation between the declarative stage and the tail.
///
/// `schema` names the PostgreSQL schema both sides are materialised into and
/// introspected over, so a project keeping its relations outside `public`
/// compares the catalogs it really declares instead of an empty `public`; the
/// selection is the one the surrounding commands resolve, and a SQLite scratch
/// holds a single unnamed catalog that ignores it.
///
/// `scratch_url` names the scratch database both sides are materialised in; see
/// [`GoldenEquation`] for its emptiness contract. A commit an interrupted process
/// left halfway is repaired first, so the boundary snapshot loaded next to
/// `checkpoint_path` is always the one the live marker describes. When
/// `--base-stage-dir` is omitted, that snapshot is loaded as the base stage, so
/// the boundary the tail replays over is the one the toolchain itself wrote.
///
/// The repair and the artifact reads run inside the exclusive `checkpoint.lock`
/// section [`CheckpointLock`] guards, so a verification that overlaps a commit
/// waits for it and reads exactly the committed pair instead of a half-swapped
/// one; a lock a provably live writer holds is never taken over, so the
/// verification refuses with a diagnostic naming that owner instead of repairing
/// a commit still in flight, while a lock whose owner is gone is taken over and
/// the commit it interrupted is repaired here. The section is released as soon as
/// the artifacts are read, because the scratch materialisation below writes no
/// workspace artifact: each scratch database is per-verification and serially
/// safe on its own.
pub(crate) fn verify_stage(
    stage_dir: &Path,
    base_stage_dir: Option<&Path>,
    migrations_dir: &Path,
    checkpoint_path: &Path,
    schema: &str,
    scratch_url: &str,
) -> Result<CommandOutput, CliError> {
    // The section guards the repair and the reads of the marker, the boundary,
    // and the tail; it is released below, before the equation materialises
    // anything, because a verification writes no workspace artifact beyond the
    // repair itself.
    let section = CheckpointLock::acquire(checkpoint_path)?;

    // A commit an interrupted process left halfway is repaired before the
    // boundary is read, so the snapshot loaded below is the one the live marker
    // describes. The repair never classifies a parked pair while a provably live
    // writer holds the section, so a commit still in flight is not undone here.
    //
    // The repair has already renamed artifacts into place by the time any read
    // below can fail, so every one of them folds its report into the failure it
    // returns: the failure channel is the only output a failing command
    // guarantees.
    let recovery = recover_interrupted_commit(checkpoint_path)?;
    let fail = |error: CliError| carry_recovery(recovery.as_deref(), error);

    let stage = load_stage(stage_dir).map_err(fail)?;
    let tail = load_migrations_from_dir(migrations_dir)
        .map_err(cli_error)
        .map_err(fail)?;
    let checkpoint = Checkpoint::load(checkpoint_path)
        .map_err(cli_error)
        .map_err(fail)?;

    // An explicit `--base-stage-dir` overrides the recorded snapshot; without
    // one, the boundary snapshot `stage commit` wrote next to the marker is
    // loaded, and an absent snapshot leaves the base unset so the golden equation
    // itself rejects a checkpoint that cannot be replayed from nothing.
    let base = match base_stage_dir {
        Some(dir) => Some(load_stage(dir).map_err(fail)?),
        None => load_recorded_boundary(&boundary_dir(checkpoint_path)).map_err(fail)?,
    };

    // The section ends here: the artifacts are read, and the equation below only
    // materialises scratch databases over files nobody writes, so the lock is not
    // held while it runs.
    drop(section);

    let mut equation = match &base {
        Some(base) => GoldenEquation::new(&stage, &tail)
            .with_schema(schema)
            .with_base(base),
        None => GoldenEquation::new(&stage, &tail).with_schema(schema),
    };
    if let Some(checkpoint) = &checkpoint {
        equation = equation.with_checkpoint(checkpoint);
    }
    let report = equation
        .verify(scratch_url)
        .map_err(cli_error)
        .map_err(fail)?;

    // The notices travel in the report of the command, so a selection that
    // describes nothing reaches the operator whether or not a tracing sink is
    // installed: an equation over an empty catalog compares nothing against
    // nothing, and certifying that silently is the false pass the detection
    // exists to prevent.
    let mut output = CommandOutput::new();
    output.notice(recovery);
    output.notices(report.schema_notices.iter().cloned());
    output.line(format!(
        "Golden equation holds: State({}) == State(Checkpoint + {}) (hash {}, schema `{schema}`, \
         scratch `{}`)",
        stage_dir.display(),
        migrations_dir.display(),
        report.stage_hash(),
        redact_database_url(scratch_url)
    ));
    Ok(output)
}

/// Loads the boundary snapshot `stage commit` recorded next to the marker.
///
/// An absent boundary directory is swept of the staging debris an interrupted
/// commit left behind; a parked snapshot that still held the only copy of the
/// boundary has already been restored by [`recover_interrupted_commit`], so a
/// verification only ever loads the snapshot the live marker describes.
fn load_recorded_boundary(dir: &Path) -> Result<Option<DeclarativeStage>, CliError> {
    if path_exists(dir)? {
        return DeclarativeStage::load(dir).map_err(cli_error);
    }

    match sweep_stale_temporaries(dir, STALE_TEMPORARY_AGE) {
        Ok(removed) => {
            for path in removed {
                tracing::debug!(
                    artifact = %path.display(),
                    "discarded a staged artifact an interrupted commit left behind"
                );
            }
        }
        Err(error) => tracing::warn!(
            error = %error,
            "failed to sweep the staged artifacts of the boundary snapshot"
        ),
    }

    DeclarativeStage::load(dir).map_err(cli_error)
}

/// Removes the staged siblings an interrupted commit abandoned next to its artifacts.
///
/// The sweep is opportunistic: the debris belongs to a process that is already
/// gone, so a sweep that cannot complete is reported without failing a commit
/// that is otherwise writable.
fn sweep_commit_debris(artifacts: &[&Path]) {
    for &artifact in artifacts {
        match sweep_stale_temporaries(artifact, STALE_TEMPORARY_AGE) {
            Ok(removed) => {
                for path in removed {
                    tracing::debug!(
                        artifact = %path.display(),
                        "discarded a staged artifact an interrupted commit left behind"
                    );
                }
            }
            Err(error) => tracing::warn!(
                error = %error,
                "failed to sweep the staged artifacts of an interrupted commit"
            ),
        }
    }
}

/// Seals the live database as the new declarative stage anchor.
///
/// The boundary is what the anchored tail replays over, so the declarative stage
/// is snapshotted verbatim into the `checkpoint-stage/` directory next to the
/// marker: the scripts keep their names and their comments, which is where the
/// replay re-reads their transactionality from. `schema` names the catalog the
/// committed snapshot and the anchored hash describe, so a project keeping its
/// relations outside `public` commits the digest of its own tables.
///
/// The declarative stage and the live catalog are compared before anything is
/// written: the lock and the marker describe the database while the boundary
/// snapshot replays the stage, so committing the two while they disagree would
/// seal a triple that contradicts itself and make [`verify_stage`] fail on a
/// checkpoint no later commit can repair.
///
/// The lock, the boundary, and the marker land in that order, but the marker is
/// the commit point: [`seal_commit`] parks the previous marker and boundary and
/// discards them only once the new marker is durable, so a process killed
/// mid-commit leaves a parked pair [`recover_interrupted_commit`] restores instead
/// of a fresh boundary described by a stale marker. The lock is not parked, because
/// it describes the live catalog the commit read and the next commit recomputes it.
///
/// The commit is split into a preparation phase and a minimal critical section,
/// because the section is bounded by the liveness budget of the checkpoint lock
/// while the preparation is not. Loading the stage, reading the live catalog over
/// the network, materialising the stage in a parity scratch database, rendering
/// the lock and the marker, and staging the boundary snapshot all take as long as a
/// remote server, a large stage, and slow storage take, and none of them mutates a
/// workspace artifact; they therefore run before the section is taken, so two
/// writers may prepare at once and the section covers local filesystem work alone.
/// The staged snapshot stays invisible under the live path until the swap renames it
/// into place, and [`StagedCleanup`] discards it on every failure in between.
///
/// The preparation observed the marker before it began, so the section re-observes
/// it: the debris sweep, the repair, the rewind guard, the `schema.lock`
/// publication, and the swap itself run inside the exclusive `checkpoint.lock`
/// section [`CheckpointLock`] guards, so a writer that committed while this one
/// prepared cannot have its boundary rewound by this commit, and a second writer
/// either waits for the section or fails naming it, never interleaving with the
/// artifacts below.
pub(crate) fn commit_stage(
    version: MigrationVersion,
    database_url: &str,
    schema: &str,
    stage_dir: &Path,
    lock_path: &Path,
    checkpoint_path: &Path,
) -> Result<CommandOutput, CliError> {
    let boundary = boundary_dir(checkpoint_path);

    // Preparation runs outside the section: it reads the stage and the database
    // but writes no workspace artifact, and it takes as long as the server and the
    // disk take.
    let stage = load_stage(stage_dir)?;
    let (catalog, notice) = introspect_database_schema_notice(database_url, schema)?;

    // The parity gate runs before the section is ever taken: the lock and the
    // marker describe the live database while the boundary snapshot replays the
    // declarative stage, so a stage that drifted from the database would seal a
    // triple that contradicts itself and make `stage verify` fail on a checkpoint
    // no later commit could repair.
    // The live catalog was read over the selected schema, so the stage has to be
    // materialised and introspected over the very same one: comparing a stage
    // built in `public` against a catalog read elsewhere would reject a
    // consistent non-public project for a drift it never had.
    let diff = compare_stage_with_live(&stage, &catalog, parity_scratch_url(database_url), schema)
        .map_err(cli_error)?;
    if !diff.is_empty() {
        return Err(stage_parity_failure(stage_dir, &diff));
    }

    let lock = catalog.to_lock();
    let checkpoint = Checkpoint::new(version, catalog.checksum());
    let marker = checkpoint.render();

    // The staged boundary is assembled here, beside the artifact it replaces, so
    // the section below only has to rename it into place.
    let staged = create_staging_dir(&boundary)?;
    if let Err(error) = write_staged_boundary(&stage, &staged) {
        let _ = std::fs::remove_dir_all(&staged);
        return Err(error);
    }

    // Flushing the staging directory makes the staged names durable before the
    // swap publishes them, so the installed boundary survives a power loss.
    if let Err(error) = sync_directory(&staged) {
        let _ = std::fs::remove_dir_all(&staged);
        return Err(error);
    }

    // From here on every failure discards the staged sibling: the swap renames it
    // away on success, so the guard is a no-op there.
    let _staged = StagedCleanup(staged.clone());

    // The section is bounded by local filesystem work alone: the sweep, the
    // repair, the rewind guard, the `schema.lock` publication, and the swap.
    let section = CheckpointLock::acquire(checkpoint_path)?;

    // Debris of an interrupted commit is discarded first, so the artifacts
    // written below never share a directory with the staged siblings a killed
    // process left behind for good.
    sweep_commit_debris(&[lock_path, checkpoint_path, boundary.as_path()]);

    // A commit an earlier process left halfway is repaired before the marker is
    // read, so the rewind guard below compares against the boundary that really
    // stands.
    //
    // The repair has already renamed artifacts into place by the time the rewind
    // guard, the lock write, or the swap can fail, so every one of them folds its
    // report into the failure it returns: the failure channel is the only output a
    // failing command guarantees.
    let recovery = recover_interrupted_commit(checkpoint_path)?;
    let fail = |error: CliError| carry_recovery(recovery.as_deref(), error);

    // The marker is re-read inside the section, so a writer that committed while
    // this one prepared cannot have its boundary rewound here.
    if let Some(existing) = Checkpoint::load(checkpoint_path)
        .map_err(cli_error)
        .map_err(fail)?
    {
        // The version this commit anchors is the one the rendered checkpoint
        // already carries, so the guard reads it from there instead of holding a
        // second copy the preparation phase would have to keep alive across the move.
        if existing.pruned_up_to_version() > checkpoint.pruned_up_to_version() {
            return Err(fail(CliError(format!(
                "`{}` already anchors the history up to version {}; rewinding it to {} \
                 would re-open pruned migrations",
                checkpoint_path.display(),
                existing.pruned_up_to_version(),
                checkpoint.pruned_up_to_version()
            ))));
        }
    }

    // The lock describes the live catalog the commit seals and is written first;
    // the marker and boundary are then swapped as one pair, with the marker
    // written last as the commit point.
    write_file(lock_path, &lock).map_err(fail)?;
    // The parked pair this commit writes carries the scope of the section it
    // holds, so a copy it leaves behind stays attributable to this writer.
    let swap_notices = seal_commit(&staged, checkpoint_path, &marker, section.writer())
        .map_err(fail)?;

    // The notices travel in the report of the command, so a selection that
    // describes nothing, a repaired commit, and a boundary whose directory could
    // not be flushed reach the operator whether or not a tracing sink is installed.
    let mut output = CommandOutput::new();
    output.notice(recovery);
    output.notice(notice);
    output.notices(swap_notices);
    output.line(format!(
        "Committed the stage to {}, snapshotted the boundary to {}, and anchored `{}` up to \
         version {} (hash {}, schema `{schema}`)",
        lock_path.display(),
        boundary.display(),
        checkpoint_path.display(),
        checkpoint.pruned_up_to_version(),
        catalog.schema_hash()
    ));
    Ok(output)
}

/// Discards a staged boundary snapshot a commit never installed.
///
/// The snapshot is assembled beside the boundary it replaces, so it stays invisible
/// under the live path until [`seal_commit`] renames it into place; a commit that
/// fails between the staging and the swap would otherwise leave a whole directory of
/// declarative DDL behind that no command would ever read. The guard removes it on
/// every failure path and is a no-op on success, where the rename has already moved
/// the directory away.
struct StagedCleanup(PathBuf);

impl Drop for StagedCleanup {
    fn drop(&mut self) {
        match std::fs::remove_dir_all(&self.0) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => tracing::warn!(
                staged = %self.0.display(),
                %error,
                "failed to discard the staged boundary snapshot"
            ),
        }
    }
}

/// The disposable state the declarative stage is materialised in before a commit.
///
/// A PostgreSQL target keeps the stage on the server behind the URL, where
/// [`compare_stage_with_live`] builds it in a throwaway database through
/// `materialise_stage`; a SQLite target is materialised in memory instead, because
/// the URL names the very file the commit is sealing and no declarative DDL may be
/// replayed over it.
fn parity_scratch_url(database_url: &str) -> &str {
    match DatabaseTarget::parse(database_url) {
        DatabaseTarget::Postgres => database_url,
        DatabaseTarget::SqliteMemory | DatabaseTarget::SqliteFile(_) => ":memory:",
    }
}

/// The heading a repaired interrupted commit is reported under once a command fails.
///
/// A repair renames checkpoint artifacts into place long before the command it
/// belongs to finishes, so a later failure has to say that the workspace already
/// changed underneath the command; the heading states that side effect plainly,
/// and the report of the repair itself follows it.
const RECOVERY_NOTE: &str =
    "note: this command repaired an interrupted commit before it failed, so the workspace already \
     changed underneath it; the repair reported:";

/// Folds the report of a repaired interrupted commit into a later failure.
///
/// [`recover_interrupted_commit`] restores or discards the parked marker and
/// boundary pair before the command does any of its own work, and the command goes
/// on through several fallible steps afterwards. Returning that report on the
/// success path alone would drop it exactly when it matters most: the failure
/// channel is the only output a command guarantees once it errors, so the report
/// travels inside the error instead of beside it.
fn carry_recovery(recovery: Option<&str>, error: CliError) -> CliError {
    let Some(recovery) = recovery else {
        return error;
    };

    CliError(format!("{}\n{RECOVERY_NOTE}\n{recovery}", error.0))
}

/// Renders the failure of a commit whose declarative stage disagrees with the database.
///
/// The findings are rendered with the wording of a commit — the stage against the
/// database — and the remedy names both descriptions, because re-running the very
/// same commit is exactly what a bare failure would invite. The gate runs during the
/// preparation of the commit, before the checkpoint section is taken, so nothing was
/// written and nothing was repaired: the artifacts of this commit and the parked
/// copies of an interrupted one are all untouched.
fn stage_parity_failure(stage_dir: &Path, diff: &SchemaDiff) -> CliError {
    CliError(format!(
        "the declarative stage `{}` does not describe the database this commit would anchor:\n{}Hint: \
         bring `schema/` and the database back into agreement — apply the migration that closes \
         the gap, or re-init the stage from the live catalog — then re-run `stage commit`. The gate \
         runs before the commit enters its checkpoint section, so it wrote no artifact and repaired \
         none: `schema.lock`, `checkpoint.toml`, the boundary snapshot, and the parked copies of an \
         interrupted commit are untouched by it",
        stage_dir.display(),
        StageFindings(diff)
    ))
}

/// The findings of a commit-level parity check rendered with its own wording.
struct StageFindings<'a>(&'a SchemaDiff);

impl fmt::Display for StageFindings<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.write_details(f, DiffWording::COMMIT)
    }
}
