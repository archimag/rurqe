//! Declarative stage and checkpoint lifecycle commands.

use std::path::Path;

use rurqe_migrate::checkpoint::Checkpoint;
use rurqe_migrate::runner::MigrationBackend;
use rurqe_migrate::stage::{cold_start, DeclarativeStage, StageMarker};
use rurqe_migrate::version::MigrationVersion;

use crate::cli::{redact_database_url, CliError};
use crate::introspect::introspect_database;
use crate::loader::load_migrations_from_dir;
use crate::stage::{materialise_stage, GoldenEquation};

use super::{cli_error, write_file};

/// Loads the declarative stage from `dir`, rejecting an absent directory.
fn load_stage(dir: &Path) -> Result<DeclarativeStage, CliError> {
    DeclarativeStage::load(dir).map_err(cli_error)?.ok_or_else(|| {
        CliError(format!(
            "declarative stage `{}` does not exist; it holds the `*.sql` DDL of the target schema",
            dir.display()
        ))
    })
}

/// Initialises a cold database from the declarative stage.
///
/// The stage is materialised in the disposable database named by `scratch_url`
/// and introspected first, so the recorded [`StageMarker`] anchors the canonical
/// catalog digest — the same digest `schema.lock` and `checkpoint.toml` carry —
/// instead of the raw DDL digest of the stage directory. An absent
/// `scratch_url` replays the stage in memory.
pub(crate) fn initialise_stage<B: MigrationBackend>(
    backend: &mut B,
    stage_dir: &Path,
    version: MigrationVersion,
    description: &str,
    scratch_url: Option<&str>,
) -> Result<(), CliError> {
    let stage = load_stage(stage_dir)?;

    if stage.is_empty() {
        return Err(CliError(format!(
            "declarative stage `{}` holds no `*.sql` script to apply",
            stage_dir.display()
        )));
    }

    let catalog =
        materialise_stage(&stage, scratch_url.unwrap_or(":memory:")).map_err(cli_error)?;

    let marker = StageMarker::new(version, description, catalog.checksum());
    let report = cold_start(backend, &stage, &marker).map_err(cli_error)?;

    if report.stage_scripts.is_empty() {
        println!(
            "Database already initialised at stage {}; nothing to apply",
            marker.version()
        );
        return Ok(());
    }

    println!(
        "Initialised the database from {} script(s) at stage {} in {} ms",
        report.stage_scripts.len(),
        marker.version(),
        report.total_time_ms
    );
    for script in &report.stage_scripts {
        println!("  - {script}");
    }
    Ok(())
}

/// Verifies the golden equation between the declarative stage and the tail.
///
/// `scratch_url` names the scratch database both sides are materialised in; see
/// [`GoldenEquation`](crate::stage::GoldenEquation) for its emptiness contract.
pub(crate) fn verify_stage(
    stage_dir: &Path,
    base_stage_dir: Option<&Path>,
    migrations_dir: &Path,
    scratch_url: &str,
) -> Result<(), CliError> {
    let stage = load_stage(stage_dir)?;
    let base = base_stage_dir.map(load_stage).transpose()?;
    let tail = load_migrations_from_dir(migrations_dir).map_err(cli_error)?;

    let equation = match &base {
        Some(base) => GoldenEquation::new(&stage, &tail).with_base(base),
        None => GoldenEquation::new(&stage, &tail),
    };
    let report = equation.verify(scratch_url).map_err(cli_error)?;

    println!(
        "Golden equation holds: State({}) == State(Checkpoint + {}) (hash {}, scratch `{}`)",
        stage_dir.display(),
        migrations_dir.display(),
        report.stage_hash(),
        redact_database_url(scratch_url)
    );
    Ok(())
}

/// Seals the live database as the new declarative stage anchor.
pub(crate) fn commit_stage(
    version: MigrationVersion,
    database_url: &str,
    lock_path: &Path,
    checkpoint_path: &Path,
) -> Result<(), CliError> {
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

    let catalog = introspect_database(database_url)?;
    write_file(lock_path, &catalog.to_lock())?;

    let checkpoint = Checkpoint::new(version, catalog.checksum());
    write_file(checkpoint_path, &checkpoint.render())?;

    println!(
        "Committed the stage to {} and anchored `{}` up to version {} (hash {})",
        lock_path.display(),
        checkpoint_path.display(),
        checkpoint.pruned_up_to_version(),
        catalog.schema_hash()
    );
    Ok(())
}
