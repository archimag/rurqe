//! Declarative stage, checkpoint, and scratch lifecycle commands.

use std::path::{Path, PathBuf};
use std::time::Duration;

use rurqe_migrate::checkpoint::Checkpoint;
use rurqe_migrate::runner::MigrationBackend;
use rurqe_migrate::stage::{cold_start, DeclarativeStage, StageMarker};
use rurqe_migrate::version::MigrationVersion;

use crate::cli::{connect, redact_database_url, CliError, CommandOutput, Connection};
use crate::introspect::introspect_database_schema_notice;
use crate::loader::load_migrations_from_dir;
use crate::stage::postgres::{
    current_owner_tag, drop_stale_scratch_databases, find_stale_scratch_databases,
    ScratchAdminSession,
};
use crate::stage::GoldenEquation;
use crate::target::{DatabaseDialect, DatabaseTarget};

use super::{
    artifact_directory, cli_error, sweep_stale_temporaries, sync_directory, temporary_sibling,
    write_file, STALE_TEMPORARY_AGE,
};

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
/// `scratch_url` names the scratch database both sides are materialised in; see
/// [`GoldenEquation`] for its emptiness contract. When `--base-stage-dir` is
/// omitted, the boundary snapshot `stage commit` recorded next to
/// `checkpoint_path` is loaded as the base stage, so the boundary the tail
/// replays over is the one the toolchain itself wrote. A snapshot an interrupted
/// commit parked under its backup name is named in the failure together with the
/// rename that restores it.
pub(crate) fn verify_stage(
    stage_dir: &Path,
    base_stage_dir: Option<&Path>,
    migrations_dir: &Path,
    checkpoint_path: &Path,
    scratch_url: &str,
) -> Result<CommandOutput, CliError> {
    let stage = load_stage(stage_dir)?;
    let tail = load_migrations_from_dir(migrations_dir).map_err(cli_error)?;
    let checkpoint = Checkpoint::load(checkpoint_path).map_err(cli_error)?;

    // An explicit `--base-stage-dir` overrides the recorded snapshot; without
    // one, the snapshot `stage commit` wrote next to the marker is loaded, and
    // an absent snapshot leaves the base unset so the golden equation itself
    // rejects a checkpoint that cannot be replayed from nothing. A snapshot an
    // interrupted commit parked under its backup name is reported with the
    // rename that restores it.
    let base = match base_stage_dir {
        Some(dir) => Some(load_stage(dir)?),
        None => load_recorded_boundary(&boundary_dir(checkpoint_path))?,
    };

    let mut equation = match &base {
        Some(base) => GoldenEquation::new(&stage, &tail).with_base(base),
        None => GoldenEquation::new(&stage, &tail),
    };
    if let Some(checkpoint) = &checkpoint {
        equation = equation.with_checkpoint(checkpoint);
    }
    let report = equation.verify(scratch_url).map_err(cli_error)?;

    Ok(CommandOutput::single(format!(
        "Golden equation holds: State({}) == State(Checkpoint + {}) (hash {}, scratch `{}`)",
        stage_dir.display(),
        migrations_dir.display(),
        report.stage_hash(),
        redact_database_url(scratch_url)
    )))
}

/// Loads the boundary snapshot `stage commit` recorded next to the marker.
///
/// An absent boundary directory is swept of the staging debris an interrupted
/// commit left behind, and a parked backup still holding the only copy of the
/// boundary is named together with the rename that restores it, so a swap the
/// operating system killed between its two renames is recoverable without an
/// operator guessing a hidden path.
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

    let backup = boundary_backup(dir);
    if path_exists(&backup)? {
        return Err(CliError(format!(
            "the boundary snapshot `{}` is missing while `{}` still holds the snapshot an \
             interrupted commit parked there; restore it with `mv {} {}`",
            dir.display(),
            backup.display(),
            backup.display(),
            dir.display()
        )));
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
/// is snapshotted verbatim into [`CHECKPOINT_STAGE_DIR`] next to the marker
/// before the marker itself is written: the scripts keep their names and their
/// comments, which is where the replay re-reads their transactionality from.
/// `schema` names the catalog the committed snapshot and the anchored hash
/// describe, so a project keeping its relations outside `public` commits the
/// digest of its own tables.
pub(crate) fn commit_stage(
    version: MigrationVersion,
    database_url: &str,
    schema: &str,
    stage_dir: &Path,
    lock_path: &Path,
    checkpoint_path: &Path,
) -> Result<CommandOutput, CliError> {
    // Debris of an interrupted commit is discarded first, so the artifacts
    // written below never share a directory with the staged siblings a killed
    // process left behind for good.
    let boundary = boundary_dir(checkpoint_path);
    sweep_commit_debris(&[lock_path, checkpoint_path, boundary.as_path()]);

    if let Some(existing) = Checkpoint::load(checkpoint_path).map_err(cli_error)? {
        if existing.pruned_up_to_version() > &version {
            return Err(CliError(format!(
                "`{}` already anchors the history up to version {}; rewinding it to {version} \
                 would re-open pruned migrations",
                checkpoint_path.display(),
                existing.pruned_up_to_version()
            )));
        }
    }

    let stage = load_stage(stage_dir)?;
    let (catalog, notice) = introspect_database_schema_notice(database_url, schema)?;

    // All artifacts are rendered before the first byte is written, and they land
    // in the order lock, snapshot, marker, so an interrupted commit never leaves
    // a checkpoint pointing at a state it cannot validate.
    let lock = catalog.to_lock();
    let checkpoint = Checkpoint::new(version, catalog.checksum());

    write_file(lock_path, &lock)?;
    let boundary_notices = snapshot_boundary_stage(&stage, checkpoint_path)?;
    write_file(checkpoint_path, &checkpoint.render())?;

    // The notices travel in the report of the command, so a selection that
    // describes nothing and a boundary whose directory could not be flushed reach
    // the operator whether or not a tracing sink is installed.
    let mut output = CommandOutput::new();
    output.notice(notice);
    output.notices(boundary_notices);
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

/// The directory holding the verbatim DDL of a committed stage boundary.
const CHECKPOINT_STAGE_DIR: &str = "checkpoint-stage";

/// The suffix the previous boundary is parked under while a swap installs a new one.
const PARKED_BOUNDARY_SUFFIX: &str = ".previous";

/// The directory the boundary snapshot of the marker at `checkpoint_path` lives in.
fn boundary_dir(checkpoint_path: &Path) -> PathBuf {
    artifact_directory(checkpoint_path).join(CHECKPOINT_STAGE_DIR)
}

/// The deterministic path the swapped-out boundary is parked at.
///
/// The name is fixed instead of process-tagged, so a commit the operating system
/// killed between the two renames of a swap leaves the only copy of the committed
/// boundary at a path a diagnostic can name and an operator can rename back.
fn boundary_backup(dir: &Path) -> PathBuf {
    let name = dir
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();

    artifact_directory(dir).join(format!(".{name}{PARKED_BOUNDARY_SUFFIX}"))
}

/// Whether `path` exists, reporting a path that cannot be inspected.
fn path_exists(path: &Path) -> Result<bool, CliError> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(CliError(format!(
            "failed to inspect `{}`: {error}",
            path.display()
        ))),
    }
}

/// Writes the verbatim DDL of the committed `stage` next to the checkpoint.
///
/// The scripts are gathered in a staging directory first and installed with a
/// single rename, so `checkpoint-stage/` always holds one whole boundary: an
/// interrupted commit leaves either the previous snapshot or the new one, never a
/// mixture of the two.
fn snapshot_boundary_stage(
    stage: &DeclarativeStage,
    checkpoint_path: &Path,
) -> Result<Vec<String>, CliError> {
    let dir = boundary_dir(checkpoint_path);
    let staged = create_staging_dir(&dir)?;

    if let Err(error) = write_staged_boundary(stage, &staged) {
        let _ = std::fs::remove_dir_all(&staged);
        return Err(error);
    }

    // Every staged script is flushed by `write_file` before it is renamed into
    // the staging directory; flushing the directory itself makes their names
    // durable too, so the swap below can only ever publish a boundary whose
    // contents survive a power loss.
    if let Err(error) = sync_directory(&staged) {
        let _ = std::fs::remove_dir_all(&staged);
        return Err(error);
    }

    swap_snapshot(&dir, &staged)
}

/// Creates the empty staging directory the new snapshot is assembled in.
///
/// A directory left behind by an interrupted attempt of a recycled process is
/// discarded instead of merged into, so two commits can never end up in one
/// snapshot.
fn create_staging_dir(dir: &Path) -> Result<PathBuf, CliError> {
    let staged = temporary_sibling(dir);

    match std::fs::remove_dir_all(&staged) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(CliError(format!(
                "failed to replace the staging directory `{}`: {error}",
                staged.display()
            )))
        }
    }

    std::fs::create_dir_all(&staged).map_err(|error| {
        CliError(format!(
            "failed to prepare the staging directory `{}`: {error}",
            staged.display()
        ))
    })?;

    Ok(staged)
}

/// Writes every script of `stage` into the staging directory `staged`.
///
/// Each script lands through the atomic [`write_file`] discipline, so a failure
/// among them only ever touches the staging directory: the committed boundary is
/// left exactly as it was.
pub(super) fn write_staged_boundary(
    stage: &DeclarativeStage,
    staged: &Path,
) -> Result<(), CliError> {
    for script in stage.scripts() {
        write_file(&staged.join(script.name()), script.sql())?;
    }

    Ok(())
}

/// Installs the staged snapshot of `staged` at `dir`, keeping the previous one whole.
///
/// The installed boundary is complete once the renames are done, so a durability gap
/// of the swap — a superseded snapshot that cannot be discarded, a directory that
/// cannot be flushed — is returned as a notice for the command's report instead of
/// failing a commit that already happened.
///
/// The previous boundary is parked at [`boundary_backup`] before the new one
/// takes its place and is renamed back when that move fails, so a reader never
/// observes a mixture of two boundaries. A parked backup found without a live
/// boundary is a swap the operating system killed between the two renames: the
/// parked snapshot is then the only copy of the committed boundary and is
/// restored before the new one is installed. The previous snapshot is discarded
/// only once the swap succeeded: failing the commit there would leave
/// `checkpoint.toml` describing a boundary the directory no longer holds.
///
/// Every rename of a swap runs inside the directory that holds `dir`: `staged` is
/// assembled by [`temporary_sibling`], and [`boundary_backup`] names a sibling of
/// `dir`, so all three moves stay within one directory and cannot cross a
/// filesystem boundary. A configuration that still reaches a device boundary fails
/// like any other rename, and [`move_failure`] reports it together with the fate of
/// the parked boundary.
pub(super) fn swap_snapshot(dir: &Path, staged: &Path) -> Result<Vec<String>, CliError> {
    let backup = boundary_backup(dir);
    let mut notices: Vec<String> = Vec::new();

    match restore_parked_boundary(dir, &backup) {
        Ok(notice) => notices.extend(notice),
        Err(error) => {
            let _ = std::fs::remove_dir_all(staged);
            return Err(error);
        }
    }

    let previous = match std::fs::rename(dir, &backup) {
        Ok(()) => Some(backup),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => {
            let _ = std::fs::remove_dir_all(staged);
            return Err(CliError(format!(
                "failed to move the previous boundary snapshot `{}` aside: {error}",
                dir.display()
            )));
        }
    };

    if let Err(error) = std::fs::rename(staged, dir) {
        let failure = move_failure(dir, staged, &error, previous.as_deref());
        let _ = std::fs::remove_dir_all(staged);
        return Err(failure);
    }

    if let Some(previous) = previous {
        notices.extend(discard_previous_snapshot(&previous));
    }

    notices.extend(flush_boundary_parent(dir));

    Ok(notices)
}

/// Reports a staged snapshot that could not be installed over `dir`.
///
/// The parked boundary is the only whole copy of the boundary while the staged one
/// is not in place, so it is renamed back before the failure is reported. A rollback
/// that fails as well strands that copy under the parked name, which no other
/// diagnostic would ever mention, so its error is folded into the same message
/// instead of being dropped: one report names both paths and the rename that
/// restores the boundary by hand.
pub(super) fn move_failure(
    dir: &Path,
    staged: &Path,
    error: &std::io::Error,
    previous: Option<&Path>,
) -> CliError {
    let mut message = format!(
        "failed to move the staged boundary snapshot `{}` into `{}`: {error}",
        staged.display(),
        dir.display()
    );

    if let Some(previous) = previous {
        if let Err(rollback) = std::fs::rename(previous, dir) {
            message.push_str(&format!(
                "; restoring the previous boundary from `{}` also failed ({rollback}), so that \
                 copy stays parked there: `mv {} {}` puts it back",
                previous.display(),
                previous.display(),
                dir.display()
            ));
        }
    }

    CliError(message)
}

/// Puts a boundary an interrupted swap parked back in place.
///
/// A backup found beside a live boundary is debris of a swap that already
/// published its replacement, so it is discarded; one found without a live
/// boundary is restored, because the parked snapshot is then the only copy of
/// the committed boundary.
fn restore_parked_boundary(dir: &Path, backup: &Path) -> Result<Option<String>, CliError> {
    if !path_exists(backup)? {
        return Ok(None);
    }

    if path_exists(dir)? {
        return Ok(discard_previous_snapshot(backup));
    }

    std::fs::rename(backup, dir).map_err(|error| {
        CliError(format!(
            "failed to restore the boundary snapshot `{}` parked at `{}`: {error}",
            dir.display(),
            backup.display()
        ))
    })?;

    Ok(None)
}

/// Discards the previous boundary once the new snapshot is in place.
///
/// The swap already succeeded, so a backup that cannot be removed is debris only:
/// reporting it would abort a commit whose snapshot is already the one the marker
/// is about to describe.
fn discard_previous_snapshot(previous: &Path) -> Option<String> {
    match std::fs::remove_dir_all(previous) {
        Ok(()) => None,
        Err(error) => Some(format!(
            "warning: failed to discard the previous boundary snapshot `{}`: {error}",
            previous.display()
        )),
    }
}

/// Flushes the directory whose entries the installed snapshot was renamed into.
///
/// The rename is durable only once that directory is flushed, but the boundary
/// has already replaced the previous one: a flush that cannot be completed only
/// weakens the durability of a commit that is otherwise done, so reporting it
/// would reject a swap that did happen. The flush itself goes through the same
/// shared `sync_directory` primitive every artifact write uses, so the
/// durability discipline cannot drift between call sites.
fn flush_boundary_parent(dir: &Path) -> Option<String> {
    let parent = artifact_directory(dir);

    match sync_directory(parent) {
        Ok(()) => None,
        Err(error) => Some(format!(
            "warning: the boundary snapshot is installed but the directory `{}` holding it could \
             not be flushed, so a crash may still resurrect the previous boundary: {error}",
            parent.display()
        )),
    }
}
