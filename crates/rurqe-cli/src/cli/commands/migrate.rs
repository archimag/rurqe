//! Flyway-style history commands: apply, status, repair, and baseline.

use std::path::Path;

use rurqe_migrate::checkpoint::Checkpoint;
use rurqe_migrate::runner::{MigrationBackend, MigrationStatusKind, Migrator};
use rurqe_migrate::version::MigrationVersion;

use crate::cli::{CliError, CommandOutput};
use crate::loader::load_migrations_from_dir;

use super::cli_error;

/// Builds the migration runner guarded by the committed checkpoint, if any.
///
/// A project that pruned its folded history carries a `checkpoint.toml` marker.
/// Loading it arms the fail-safe outdated-target protection, so a database whose
/// newest recorded version sits below the pruned boundary is rejected instead of
/// being silently advanced by the remaining tail. A project without a marker
/// keeps the classic sequential lifecycle.
fn load_migrator(checkpoint_path: &Path) -> Result<Migrator, CliError> {
    match Checkpoint::load(checkpoint_path).map_err(cli_error)? {
        Some(checkpoint) => Ok(Migrator::with_checkpoint(checkpoint)),
        None => Ok(Migrator::new()),
    }
}

/// Applies every pending migration and reports what ran.
pub(crate) fn apply_migrations<B: MigrationBackend>(
    backend: &mut B,
    migrations_dir: &Path,
    checkpoint_path: &Path,
) -> Result<CommandOutput, CliError> {
    let migrations = load_migrations_from_dir(migrations_dir).map_err(cli_error)?;
    let report = load_migrator(checkpoint_path)?
        .migrate(backend, &migrations)
        .map_err(cli_error)?;

    let mut output = CommandOutput::new();
    output.line(format!(
        "Applied {} migration(s) in {} ms",
        report.applied_count, report.total_time_ms
    ));
    for script in &report.applied_scripts {
        output.line(format!("  - {script}"));
    }
    Ok(output)
}

/// Reports the status of every known migration against the recorded history.
pub(crate) fn report_status<B: MigrationBackend>(
    backend: &mut B,
    migrations_dir: &Path,
) -> Result<CommandOutput, CliError> {
    let migrations = load_migrations_from_dir(migrations_dir).map_err(cli_error)?;
    let statuses = Migrator::new()
        .status(backend, &migrations)
        .map_err(cli_error)?;

    let mut output = CommandOutput::new();

    if statuses.is_empty() {
        output.line(format!(
            "No migrations found in {}",
            migrations_dir.display()
        ));
        return Ok(output);
    }

    output.line(format!(
        "{:<40} {:<12} {:<10} STATUS",
        "SCRIPT", "KIND", "VERSION"
    ));
    for status in &statuses {
        let state = match status.status {
            MigrationStatusKind::Pending => "Pending",
            MigrationStatusKind::Applied => "Applied",
            MigrationStatusKind::Modified => "Modified",
            MigrationStatusKind::IgnoredBaseline => "IgnoredBaseline",
            MigrationStatusKind::Failed => "Failed",
        };
        let version = status
            .version
            .as_ref()
            .map(ToString::to_string)
            .unwrap_or_default();
        output.line(format!(
            "{:<40} {:<12} {:<10} {}",
            status.script,
            status.kind.as_str(),
            version,
            state
        ));
    }
    Ok(output)
}

/// Rewrites the recorded checksums and purges failed attempts.
pub(crate) fn repair_history<B: MigrationBackend>(
    backend: &mut B,
    migrations_dir: &Path,
) -> Result<CommandOutput, CliError> {
    let migrations = load_migrations_from_dir(migrations_dir).map_err(cli_error)?;
    let report = Migrator::new()
        .repair(backend, &migrations)
        .map_err(cli_error)?;

    let mut output = CommandOutput::new();
    output.line(format!(
        "Removed {} failed entr(ies), updated {} checksum(s)",
        report.removed_failed_entries,
        report.updated_checksums.len()
    ));
    for script in &report.updated_checksums {
        output.line(format!("  - {script}"));
    }
    Ok(output)
}
/// Drops every object of the target database, gated by the `--yes` confirmation.
///
/// The catalogue is read exactly once per invocation, and that one reading is
/// what the gate is for: without the confirmation the command fails with the
/// manifest the call would drop, so the list the operator sees is the list a
/// confirmed call erases, and the refusal doubles as the dry run. The URL is
/// supplied already redacted, so no message of this command echoes a credential.
pub(crate) fn clean_database<B: MigrationBackend>(
    backend: &mut B,
    database_url: &str,
    force: bool,
) -> Result<CommandOutput, CliError> {
    let migrator = Migrator::new();

    if !force {
        let manifest = migrator.clean_manifest(backend).map_err(cli_error)?;

        if manifest.is_empty() {
            return Ok(CommandOutput::single(nothing_to_clean(database_url)));
        }

        let mut message = format!(
            "`migrate clean` requires `--yes` (or `--force`) to confirm dropping every object of \
             `{database_url}`; found {} object(s):",
            manifest.len()
        );
        for name in &manifest {
            message.push_str(&format!("\n  - {name}"));
        }
        return Err(CliError(message));
    }

    let dropped = migrator.clean(backend).map_err(cli_error)?;

    if dropped.is_empty() {
        return Ok(CommandOutput::single(nothing_to_clean(database_url)));
    }

    let mut output = CommandOutput::new();
    output.line(format!("Dropped {} object(s) of `{database_url}`:", dropped.len()));
    for name in &dropped {
        output.line(format!("  - {name}"));
    }
    Ok(output)
}

/// The report of a target that holds nothing the migration lifecycle owns.
fn nothing_to_clean(database_url: &str) -> String {
    format!("Nothing to clean: `{database_url}` holds no object the migration lifecycle owns")
}
/// Records a baseline marker without running the earlier migrations.
pub(crate) fn baseline_database<B: MigrationBackend>(
    backend: &mut B,
    version: MigrationVersion,
    description: &str,
) -> Result<CommandOutput, CliError> {
    let entry = Migrator::new()
        .baseline(backend, version, description)
        .map_err(cli_error)?;
    Ok(CommandOutput::single(format!(
        "Baselined database at rank {} ({})",
        entry.installed_rank, entry.description
    )))
}
