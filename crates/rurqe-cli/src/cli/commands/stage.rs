//! Declarative stage, checkpoint, and scratch lifecycle commands.

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
pub(crate) fn verify_stage(
    stage_dir: &Path,
    base_stage_dir: Option<&Path>,
    migrations_dir: &Path,
    checkpoint_path: &Path,
    schema: &str,
    scratch_url: &str,
) -> Result<CommandOutput, CliError> {
    // A commit an interrupted process left halfway is repaired before the
    // boundary is read, so the snapshot loaded below is the one the live marker
    // describes.
    let recovery = recover_interrupted_commit(checkpoint_path)?;

    let stage = load_stage(stage_dir)?;
    let tail = load_migrations_from_dir(migrations_dir).map_err(cli_error)?;
    let checkpoint = Checkpoint::load(checkpoint_path).map_err(cli_error)?;

    // An explicit `--base-stage-dir` overrides the recorded snapshot; without
    // one, the boundary snapshot `stage commit` wrote next to the marker is
    // loaded, and an absent snapshot leaves the base unset so the golden equation
    // itself rejects a checkpoint that cannot be replayed from nothing.
    let base = match base_stage_dir {
        Some(dir) => Some(load_stage(dir)?),
        None => load_recorded_boundary(&boundary_dir(checkpoint_path))?,
    };

    let mut equation = match &base {
        Some(base) => GoldenEquation::new(&stage, &tail)
            .with_schema(schema)
            .with_base(base),
        None => GoldenEquation::new(&stage, &tail).with_schema(schema),
    };
    if let Some(checkpoint) = &checkpoint {
        equation = equation.with_checkpoint(checkpoint);
    }
    let report = equation.verify(scratch_url).map_err(cli_error)?;

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
/// is snapshotted verbatim into [`CHECKPOINT_STAGE_DIR`] next to the marker: the
/// scripts keep their names and their comments, which is where the replay
/// re-reads their transactionality from. `schema` names the catalog the committed
/// snapshot and the anchored hash describe, so a project keeping its relations
/// outside `public` commits the digest of its own tables.
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
pub(crate) fn commit_stage(
    version: MigrationVersion,
    database_url: &str,
    schema: &str,
    stage_dir: &Path,
    lock_path: &Path,
    checkpoint_path: &Path,
) -> Result<CommandOutput, CliError> {
    let boundary = boundary_dir(checkpoint_path);

    // Debris of an interrupted commit is discarded first, so the artifacts
    // written below never share a directory with the staged siblings a killed
    // process left behind for good.
    sweep_commit_debris(&[lock_path, checkpoint_path, boundary.as_path()]);

    // A commit an earlier process left halfway is repaired before the boundary is
    // read, so the rewind guard and the artifacts below operate on a
    // self-consistent marker/boundary pair.
    let recovery = recover_interrupted_commit(checkpoint_path)?;

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

    // The parity gate runs before the first artifact is rendered: the lock and
    // the marker describe the live database while the boundary snapshot replays
    // the declarative stage, so a stage that drifted from the database would seal
    // a triple that contradicts itself and make `stage verify` fail on a
    // checkpoint no later commit could repair.
    // The live catalog was read over the selected schema, so the stage has to be
    // materialised and introspected over the very same one: comparing a stage
    // built in `public` against a catalog read elsewhere would reject a
    // consistent non-public project for a drift it never had.
    let diff = compare_stage_with_live(
        &stage,
        &catalog,
        parity_scratch_url(database_url),
        schema,
    )
    .map_err(cli_error)?;
    if !diff.is_empty() {
        return Err(stage_parity_failure(stage_dir, &diff));
    }

    let lock = catalog.to_lock();
    let checkpoint = Checkpoint::new(version, catalog.checksum());

    // The lock describes the live catalog the commit seals and is written first;
    // the marker and boundary are then swapped as one pair, with the marker
    // written last as the commit point.
    write_file(lock_path, &lock)?;
    let swap_notices = seal_commit(&stage, checkpoint_path, &checkpoint.render())?;

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

/// The directory holding the verbatim DDL of a committed stage boundary.
const CHECKPOINT_STAGE_DIR: &str = "checkpoint-stage";

/// The suffix a previous artifact is parked under while a commit replaces it.
///
/// The name is fixed instead of process-tagged, so a commit the operating system
/// killed mid-sequence leaves the previous marker and boundary at a path the next
/// run can find, name, and restore.
const PARKED_SUFFIX: &str = ".previous";

/// The directory the boundary snapshot of the marker at `checkpoint_path` lives in.
fn boundary_dir(checkpoint_path: &Path) -> PathBuf {
    artifact_directory(checkpoint_path).join(CHECKPOINT_STAGE_DIR)
}

/// The deterministic sibling path `artifact` is parked at during a commit.
///
/// It names a sibling of `artifact`, so parking and restoring it stays inside one
/// directory and cannot cross a filesystem boundary.
fn parked_sibling(artifact: &Path) -> PathBuf {
    let name = artifact
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();

    artifact_directory(artifact).join(format!(".{name}{PARKED_SUFFIX}"))
}

/// Whether `path` exists, reporting a path that cannot be inspected.
fn path_exists(path: &Path) -> Result<bool, CliError> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        // A path below a component that is not a directory cannot hold an entry;
        // the write that follows reports the unusable path with its artifact.
        Err(_)
            if matches!(
                path.parent(),
                Some(parent) if !parent.as_os_str().is_empty() && !names_a_directory(parent)
            ) =>
        {
            Ok(false)
        }
        Err(error) => Err(CliError(format!(
            "failed to inspect `{}`: {error}",
            path.display()
        ))),
    }
}

/// Whether `path` names an existing directory.
fn names_a_directory(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|metadata| metadata.is_dir())
}

/// Repairs a commit an interrupted process left halfway.
///
/// A commit parks the previous marker and boundary, installs the new boundary, and
/// writes the new marker last as its commit point. A process killed before the
/// commit point therefore leaves the previous marker parked and no marker of the
/// new commit live; one killed after it leaves the committed pair live with the
/// previous pair parked as debris. The live marker decides which state a run
/// inherits: its presence beside a live boundary means the commit reached its
/// commit point, so the parked pair is discarded; otherwise the parked pair is
/// restored and any boundary installed without its marker is dropped, so the
/// marker never describes a boundary another commit replaced.
///
/// Every removal and rename happens before a command reads the pair, so neither
/// `stage commit` nor `stage verify` ever observes a self-inconsistent marker and
/// boundary while a parked copy still holds the previous state.
pub(super) fn recover_interrupted_commit(checkpoint_path: &Path) -> Result<Option<String>, CliError> {
    let dir = boundary_dir(checkpoint_path);
    let marker_backup = parked_sibling(checkpoint_path);
    let boundary_backup = parked_sibling(&dir);

    if !path_exists(&marker_backup)? && !path_exists(&boundary_backup)? {
        return Ok(None);
    }

    let marker_live = path_exists(checkpoint_path)?;
    let boundary_live = path_exists(&dir)?;

    // A live marker beside a live boundary is a commit that reached its commit
    // point: the parked pair is the state its swap replaced and is discarded.
    if marker_live && boundary_live {
        let mut message = String::from(
            "Discarded the checkpoint marker and boundary an earlier commit already replaced",
        );
        for notice in [
            discard_parked(&marker_backup),
            discard_parked(&boundary_backup),
            flush_boundary_parent(&dir),
        ]
        .into_iter()
        .flatten()
        {
            message.push('\n');
            message.push_str(&notice);
        }
        return Ok(Some(message));
    }

    // Otherwise the commit stopped before its commit point: the parked pair is
    // the state the marker last described, and a boundary installed without its
    // marker is debris the parked boundary replaces.
    if path_exists(&boundary_backup)? {
        if boundary_live {
            std::fs::remove_dir_all(&dir).map_err(|error| {
                CliError(format!(
                    "failed to discard the boundary `{}` an interrupted commit installed without \
                     its marker: {error}",
                    dir.display()
                ))
            })?;
        }
        std::fs::rename(&boundary_backup, &dir).map_err(|error| {
            CliError(format!(
                "failed to restore the boundary snapshot `{}` parked at `{}`: {error}; `mv {} {}` \
                 puts it back",
                dir.display(),
                boundary_backup.display(),
                boundary_backup.display(),
                dir.display()
            ))
        })?;
    }

    if path_exists(&marker_backup)? {
        if marker_live {
            std::fs::remove_file(checkpoint_path).map_err(|error| {
                CliError(format!(
                    "failed to discard the marker `{}` an interrupted commit installed without \
                     its boundary: {error}",
                    checkpoint_path.display()
                ))
            })?;
        }
        std::fs::rename(&marker_backup, checkpoint_path).map_err(|error| {
            CliError(format!(
                "failed to restore the checkpoint marker `{}` parked at `{}`: {error}; `mv {} {}` \
                 puts it back",
                checkpoint_path.display(),
                marker_backup.display(),
                marker_backup.display(),
                checkpoint_path.display()
            ))
        })?;
    }

    let mut message = String::from(
        "Restored the checkpoint marker and boundary an interrupted commit left parked",
    );
    if let Some(notice) = flush_boundary_parent(&dir) {
        message.push('\n');
        message.push_str(&notice);
    }
    Ok(Some(message))
}

/// Installs the new boundary and marker as one commit, keeping the previous pair
/// parked until the marker is durable.
///
/// The new boundary is assembled and flushed first, so a failure up to there
/// leaves the committed pair exactly as it was. The previous marker and boundary
/// are then parked, the staged boundary is installed, and the marker is written
/// last as the commit point. Only once the marker is durable are the parked
/// artifacts discarded. A process killed before the commit point leaves the
/// previous pair parked for [`recover_interrupted_commit`] to restore; one killed
/// after it leaves the committed pair live with the previous pair parked as
/// debris.
fn seal_commit(
    stage: &DeclarativeStage,
    checkpoint_path: &Path,
    marker: &str,
) -> Result<Vec<String>, CliError> {
    let dir = boundary_dir(checkpoint_path);
    let marker_backup = parked_sibling(checkpoint_path);
    let boundary_backup = parked_sibling(&dir);

    let staged = create_staging_dir(&dir)?;

    if let Err(error) = write_staged_boundary(stage, &staged) {
        let _ = std::fs::remove_dir_all(&staged);
        return Err(error);
    }

    // Flushing the staging directory makes the staged names durable before the
    // swap publishes them, so the installed boundary survives a power loss.
    if let Err(error) = sync_directory(&staged) {
        let _ = std::fs::remove_dir_all(&staged);
        return Err(error);
    }

    // The marker is parked before the boundary: a kill between the two parks
    // leaves the boundary the parked marker describes still live, so the recovery
    // only has to put the marker back.
    let parked_marker = match park(checkpoint_path, &marker_backup) {
        Ok(parked) => parked,
        Err(error) => {
            let _ = std::fs::remove_dir_all(&staged);
            return Err(error);
        }
    };

    let parked_boundary = match park(&dir, &boundary_backup) {
        Ok(parked) => parked,
        Err(error) => {
            let _ = std::fs::remove_dir_all(&staged);
            let mut failure = error;
            if let Some(notice) = restore(checkpoint_path, parked_marker.as_deref()) {
                failure.0.push_str(&format!("; {notice}"));
            }
            return Err(failure);
        }
    };

    if let Err(error) = std::fs::rename(&staged, &dir) {
        let mut failure = move_failure(&dir, &staged, &error, parked_boundary.as_deref());
        if let Some(notice) = restore(checkpoint_path, parked_marker.as_deref()) {
            failure.0.push_str(&format!("; {notice}"));
        }
        let _ = std::fs::remove_dir_all(&staged);
        return Err(failure);
    }

    // Writing the marker is the commit point: a verification only observes the
    // pair once this file is durable, and a kill before it leaves the parked pair
    // for the recovery to restore.
    if let Err(error) = write_file(checkpoint_path, marker) {
        let _ = std::fs::remove_dir_all(&dir);
        let mut failure = error;
        if let Some(notice) = restore(&dir, parked_boundary.as_deref()) {
            failure.0.push_str(&format!("\n{notice}"));
        }
        if let Some(notice) = restore(checkpoint_path, parked_marker.as_deref()) {
            failure.0.push_str(&format!("\n{notice}"));
        }
        return Err(failure);
    }

    // The commit point is durable, so the parked pair is debris.
    let mut notices = Vec::new();
    notices.extend(discard_parked(&marker_backup));
    notices.extend(discard_parked(&boundary_backup));
    notices.extend(flush_boundary_parent(&dir));
    Ok(notices)
}

/// Parks `artifact` at `backup`, returning the backup path when one was parked.
///
/// An absent artifact is not an error: the first commit of a project parks
/// nothing, and its marker is written without a previous state to keep. A backup
/// still present here is debris an earlier recovery left behind and is discarded
/// so the rename below never fails over an existing directory.
fn park(artifact: &Path, backup: &Path) -> Result<Option<PathBuf>, CliError> {
    if let Some(notice) = discard_parked(backup) {
        tracing::warn!(%notice, "discarding a parked artifact before parking a new one");
    }

    match std::fs::rename(artifact, backup) {
        Ok(()) => Ok(Some(backup.to_owned())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(CliError(format!(
            "failed to park `{}` at `{}`: {error}",
            artifact.display(),
            backup.display()
        ))),
    }
}

/// Restores a parked artifact to its live position, if one was parked.
///
/// A restore that fails is returned as a diagnostic instead of dropped, so the
/// caller can fold it into the failure that triggered the rollback and the
/// operator learns where the parked copy sits.
fn restore(live: &Path, parked: Option<&Path>) -> Option<String> {
    let parked = parked?;

    match std::fs::rename(parked, live) {
        Ok(()) => None,
        Err(error) => Some(format!(
            "failed to restore `{}` from `{}`: {error}; `mv {} {}` puts it back",
            live.display(),
            parked.display(),
            parked.display(),
            live.display()
        )),
    }
}

/// Discards a parked artifact once the live pair is in place.
///
/// A command that reaches this point already has consistent live artifacts, so a
/// parked one that cannot be removed is debris only: reporting it would abort a
/// command whose artifacts are already the ones they should be.
fn discard_parked(parked: &Path) -> Option<String> {
    let metadata = match std::fs::symlink_metadata(parked) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return None,
        Err(error) => {
            return Some(format!(
                "warning: failed to inspect the parked artifact `{}`: {error}",
                parked.display()
            ))
        }
    };

    let removal = if metadata.is_dir() {
        std::fs::remove_dir_all(parked)
    } else {
        std::fs::remove_file(parked)
    };

    match removal {
        Ok(()) => None,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => Some(format!(
            "warning: failed to discard the parked artifact `{}`: {error}",
            parked.display()
        )),
    }
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

/// Flushes the directory whose entries the installed snapshot was renamed into.
///
/// The rename is durable only once that directory is flushed, but the boundary
/// has already replaced the previous one: a flush that cannot be completed only
/// weakens the durability of a commit that is otherwise done, so reporting it
/// would reject a swap that did happen. The flush itself goes through the same
/// shared `sync_directory` primitive every artifact write uses, so the
/// durability discipline cannot drift between call sites.
pub(super) fn flush_boundary_parent(dir: &Path) -> Option<String> {
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

/// Renders the failure of a commit whose declarative stage disagrees with the database.
///
/// The findings are rendered with the wording of a commit — the stage against the
/// database — and the remedy names both descriptions, because re-running the very
/// same commit is exactly what a bare failure would invite.
fn stage_parity_failure(stage_dir: &Path, diff: &SchemaDiff) -> CliError {
    CliError(format!(
        "the declarative stage `{}` does not describe the database this commit would anchor:\n{}Hint: \
         bring `schema/` and the database back into agreement — apply the migration that closes \
         the gap, or re-init the stage from the live catalog — then re-run `stage commit`; no \
         artifact was written",
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
