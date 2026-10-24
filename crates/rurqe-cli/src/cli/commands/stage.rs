//! Declarative stage, checkpoint, and scratch lifecycle commands.

use std::path::Path;
use std::time::Duration;

use rurqe_migrate::checkpoint::Checkpoint;
use rurqe_migrate::runner::MigrationBackend;
use rurqe_migrate::stage::{cold_start, DeclarativeStage, StageMarker};
use rurqe_migrate::version::MigrationVersion;

use crate::cli::{connect, redact_database_url, CliError, Connection};
use crate::introspect::introspect_database;
use crate::loader::load_migrations_from_dir;
use crate::stage::postgres::sweep_stale_scratch_databases;
use crate::stage::GoldenEquation;
use crate::target::{DatabaseDialect, DatabaseTarget};

use super::{cli_error, write_file};

/// Drops the PostgreSQL scratch databases earlier verifications abandoned.
///
/// Every verification against a PostgreSQL server builds its replay inside a
/// throwaway `rurqe_scratch_…` database and drops it again, so a verification
/// that was killed before it could clean up leaves one behind. The sweep dates
/// each database by the instant its name records and drops the ones older than
/// `older_than` — the same opportunistic cleanup a verification runs before it
/// creates a scratch database of its own, exposed here for an explicit run.
///
/// SQLite needs no sweep: a verification opens either an in-memory database or a
/// file the operator owns, so nothing is ever left behind on a shared server.
pub(crate) fn cleanup_stage(scratch_url: &str, older_than: Duration) -> Result<(), CliError> {
    if DatabaseTarget::parse(scratch_url).dialect() != DatabaseDialect::Postgres {
        println!(
            "Nothing to sweep: `{}` addresses a SQLite scratch database, which a verification \
             opens in memory or in the file it is pointed at",
            redact_database_url(scratch_url)
        );
        return Ok(());
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

    let dropped =
        sweep_stale_scratch_databases(&mut admin, older_than, scratch_url).map_err(cli_error)?;

    if dropped.is_empty() {
        println!(
            "No PostgreSQL scratch database on `{}` is older than {} second(s)",
            redact_database_url(scratch_url),
            older_than.as_secs()
        );
        return Ok(());
    }

    println!(
        "Dropped {} stale PostgreSQL scratch database(s) on `{}` older than {} second(s):",
        dropped.len(),
        redact_database_url(scratch_url),
        older_than.as_secs()
    );
    for database in &dropped {
        println!("  - {database}");
    }

    Ok(())
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
) -> Result<(), CliError> {
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
/// [`GoldenEquation`] for its emptiness contract.
pub(crate) fn verify_stage(
    stage_dir: &Path,
    base_stage_dir: Option<&Path>,
    migrations_dir: &Path,
    checkpoint_path: &Path,
    scratch_url: &str,
) -> Result<(), CliError> {
    let stage = load_stage(stage_dir)?;
    let base = base_stage_dir.map(load_stage).transpose()?;
    let tail = load_migrations_from_dir(migrations_dir).map_err(cli_error)?;
    let checkpoint = Checkpoint::load(checkpoint_path).map_err(cli_error)?;

    let mut equation = match &base {
        Some(base) => GoldenEquation::new(&stage, &tail).with_base(base),
        None => GoldenEquation::new(&stage, &tail),
    };
    if let Some(checkpoint) = &checkpoint {
        equation = equation.with_checkpoint(checkpoint);
    }
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

    // Both artifacts are rendered before the first byte is written, and the
    // snapshot lands before the marker that anchors it, so an interrupted commit
    // never leaves a checkpoint pointing at a lock file it cannot validate.
    let lock = catalog.to_lock();
    let checkpoint = Checkpoint::new(version, catalog.checksum());

    write_file(lock_path, &lock)?;
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
