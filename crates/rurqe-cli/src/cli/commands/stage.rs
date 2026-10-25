//! Declarative stage, checkpoint, and scratch lifecycle commands.

use std::path::{Path, PathBuf};
use std::time::Duration;

use rurqe_migrate::checkpoint::Checkpoint;
use rurqe_migrate::runner::MigrationBackend;
use rurqe_migrate::stage::{cold_start, DeclarativeStage, StageMarker};
use rurqe_migrate::version::MigrationVersion;

use crate::cli::{connect, redact_database_url, CliError, CommandOutput, Connection};
use crate::introspect::introspect_database;
use crate::loader::load_migrations_from_dir;
use crate::stage::postgres::{
    current_owner_tag, find_stale_scratch_databases, sweep_stale_scratch_databases,
};
use crate::stage::GoldenEquation;
use crate::target::{DatabaseDialect, DatabaseTarget};

use super::{cli_error, temporary_sibling, write_file};

/// Drops the PostgreSQL scratch databases earlier verifications abandoned.
///
/// Requires explicit confirmation via `force` (`--yes` / `--force`) before dropping.
/// Without confirmation, the candidates are named in the reported failure so the
/// operator sees exactly what the confirmation would drop.
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

    let owner_tag = current_owner_tag();
    let candidates =
        find_stale_scratch_databases(&mut admin, older_than, &owner_tag).map_err(cli_error)?;

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

    let dropped =
        sweep_stale_scratch_databases(&mut admin, older_than, scratch_url).map_err(cli_error)?;

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
/// introspect itself keeps the stage digest supplied here as its fallback.
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
/// replays over is the one the toolchain itself wrote.
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
    // rejects a checkpoint that cannot be replayed from nothing.
    let base = match base_stage_dir {
        Some(dir) => Some(load_stage(dir)?),
        None => DeclarativeStage::load(&boundary_dir(checkpoint_path)).map_err(cli_error)?,
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

/// The directory holding the verbatim DDL of a committed stage boundary.
const CHECKPOINT_STAGE_DIR: &str = "checkpoint-stage";

/// Seals the live database as the new declarative stage anchor.
///
/// The boundary is what the anchored tail replays over, so the declarative stage
/// is snapshotted verbatim into [`CHECKPOINT_STAGE_DIR`] next to the marker
/// before the marker itself is written: the scripts keep their names and their
/// comments, which is where the replay re-reads their transactionality from.
pub(crate) fn commit_stage(
    version: MigrationVersion,
    database_url: &str,
    stage_dir: &Path,
    lock_path: &Path,
    checkpoint_path: &Path,
) -> Result<CommandOutput, CliError> {
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
    let catalog = introspect_database(database_url)?;

    // All artifacts are rendered before the first byte is written, and they land
    // in the order lock, snapshot, marker, so an interrupted commit never leaves
    // a checkpoint pointing at a state it cannot validate.
    let lock = catalog.to_lock();
    let checkpoint = Checkpoint::new(version, catalog.checksum());

    write_file(lock_path, &lock)?;
    snapshot_boundary_stage(&stage, checkpoint_path)?;
    write_file(checkpoint_path, &checkpoint.render())?;

    Ok(CommandOutput::single(format!(
        "Committed the stage to {}, snapshotted the boundary to {}, and anchored `{}` up to \
         version {} (hash {})",
        lock_path.display(),
        boundary_dir(checkpoint_path).display(),
        checkpoint_path.display(),
        checkpoint.pruned_up_to_version(),
        catalog.schema_hash()
    )))
}

/// The directory the boundary snapshot of the marker at `checkpoint_path` lives in.
fn boundary_dir(checkpoint_path: &Path) -> PathBuf {
    checkpoint_path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
        .join(CHECKPOINT_STAGE_DIR)
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
) -> Result<(), CliError> {
    let dir = boundary_dir(checkpoint_path);
    let staged = create_staging_dir(&dir)?;

    if let Err(error) = write_staged_boundary(stage, &staged) {
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
pub(super) fn write_staged_boundary(stage: &DeclarativeStage, staged: &Path) -> Result<(), CliError> {
    for script in stage.scripts() {
        write_file(&staged.join(script.name()), script.sql())?;
    }

    Ok(())
}

/// Installs the staged snapshot of `staged` at `dir`, keeping the previous one whole.
///
/// The previous boundary is renamed aside before the new one takes its place and
/// is renamed back when that move fails, so a reader never observes a mixture of
/// two boundaries. The previous snapshot is discarded only once the swap
/// succeeded: failing the commit there would leave `checkpoint.toml` describing a
/// boundary the directory no longer holds.
pub(super) fn swap_snapshot(dir: &Path, staged: &Path) -> Result<(), CliError> {
    let backup = temporary_sibling(dir);
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
        if let Some(previous) = &previous {
            let _ = std::fs::rename(previous, dir);
        }
        let _ = std::fs::remove_dir_all(staged);
        return Err(CliError(format!(
            "failed to move the staged boundary snapshot into `{}`: {error}",
            dir.display()
        )));
    }

    if let Some(previous) = previous {
        discard_previous_snapshot(&previous);
    }

    Ok(())
}

/// Discards the previous boundary once the new snapshot is in place.
///
/// The swap already succeeded, so a backup that cannot be removed is debris only:
/// reporting it would abort a commit whose snapshot is already the one the marker
/// is about to describe.
fn discard_previous_snapshot(previous: &Path) {
    if let Err(error) = std::fs::remove_dir_all(previous) {
        tracing::warn!(
            backup = %previous.display(),
            error = %error,
            "failed to discard the previous boundary snapshot"
        );
    }
}
