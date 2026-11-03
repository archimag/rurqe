//! Flyway-style history commands: apply, status, repair, and baseline.

use std::path::Path;

use rurqe_migrate::checkpoint::Checkpoint;
use rurqe_migrate::runner::{MigrationBackend, MigrationStatusKind, Migrator};
use rurqe_migrate::version::MigrationVersion;

use crate::cli::{CliError, CommandOutput};
use crate::loader::load_migrations_from_dir;

use super::cli_error;
use super::stage::lock::CheckpointLock;

/// Builds the migration runner guarded by the committed checkpoint, if any.
///
/// A project that pruned its folded history carries a `checkpoint.toml` marker.
/// Loading it arms the fail-safe outdated-target protection, so a database whose
/// newest recorded version sits below the pruned boundary is rejected instead of
/// being silently advanced by the remaining tail. A project without a marker
/// keeps the classic sequential lifecycle.
fn load_migrator(checkpoint_path: &Path) -> Result<Migrator, CliError> {
    match load_checkpoint(checkpoint_path)? {
        Some(checkpoint) => Ok(Migrator::with_checkpoint(checkpoint)),
        None => Ok(Migrator::new()),
    }
}

/// Loads the committed checkpoint marker, if the project carries one.
///
/// The marker is read through this one function by every command that needs the
/// boundary, so its absence means one thing everywhere: a project that never
/// pruned its history. A marker that is present but unreadable stays an error,
/// because reading it as absent would hide a boundary the run obeys.
fn load_checkpoint(checkpoint_path: &Path) -> Result<Option<Checkpoint>, CliError> {
    Checkpoint::load(checkpoint_path).map_err(cli_error)
}

/// Applies every pending migration and reports what ran.
///
/// The tail and the boundary marker are shared with the writers of the checkpoint
/// artifacts, so both are read inside the exclusive [`CheckpointLock`] section of
/// the marker and the section is released before the backend is touched: a
/// concurrent `prune` waits for the read instead of deleting a script between the
/// listing and the load, while the database work keeps its own exclusive run lock
/// and no lock ordering arises between the two.
pub(crate) fn apply_migrations<B: MigrationBackend>(
    backend: &mut B,
    migrations_dir: &Path,
    checkpoint_path: &Path,
) -> Result<CommandOutput, CliError> {
    let (migrations, migrator) = {
        let _section = CheckpointLock::acquire(checkpoint_path)?;
        let migrations = load_migrations_from_dir(migrations_dir).map_err(cli_error)?;
        (migrations, load_migrator(checkpoint_path)?)
    };
    let report = migrator.migrate(backend, &migrations).map_err(cli_error)?;

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
///
/// The table states what the tail holds, not whether the tail may run. The
/// boundary marker guards the run, and every status is derived from the recorded
/// history alone, so a database the next `up` refuses is listed exactly like one
/// it advances. The boundary is therefore evaluated beside the table and reported
/// as a notice: the operator reads the refusal the run would produce instead of a
/// list of `Pending` rows that will never apply.
pub(crate) fn report_status<B: MigrationBackend>(
    backend: &mut B,
    migrations_dir: &Path,
    checkpoint_path: &Path,
) -> Result<CommandOutput, CliError> {
    // The tail and the marker are shared with the writers of the checkpoint
    // artifacts, so both are read inside the exclusive section of the marker and
    // the section is released before the backend is driven: the history read
    // keeps its own exclusive run lock, and no lock ordering arises between the
    // two.
    let (migrations, checkpoint) = {
        let _section = CheckpointLock::acquire(checkpoint_path)?;
        let migrations = load_migrations_from_dir(migrations_dir).map_err(cli_error)?;
        (migrations, load_checkpoint(checkpoint_path)?)
    };
    let statuses = Migrator::new()
        .status(backend, &migrations)
        .map_err(cli_error)?;

    // Evaluated after the statuses: `status` has just ensured the metadata table
    // exists, so the history the boundary check reads is the one a run would see
    // rather than a missing table.
    let boundary = boundary_refusal(backend, checkpoint.as_ref(), checkpoint_path)?;

    let mut output = CommandOutput::new();

    if statuses.is_empty() {
        output.line(format!(
            "No migrations found in {}",
            migrations_dir.display()
        ));
        output.notice(boundary);
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
    output.notice(boundary);
    Ok(output)
}

/// The refusal a run of the tail would produce for this database, if any.
///
/// The condition is the one [`Migrator`] evaluates over the recorded history, so
/// the report and the run it precedes cannot word the same boundary differently.
/// Only the history participates, because the live catalog of a target sitting
/// exactly at the boundary is measured by the run itself and by `stage verify`,
/// which introspect the database they are about to advance; a project without a
/// marker carries no boundary, and nothing is reported.
fn boundary_refusal<B: MigrationBackend>(
    backend: &mut B,
    checkpoint: Option<&Checkpoint>,
    checkpoint_path: &Path,
) -> Result<Option<String>, CliError> {
    let Some(checkpoint) = checkpoint else {
        return Ok(None);
    };

    let history = backend.read_history().map_err(cli_error)?;

    match checkpoint.validate_history(&history) {
        Ok(()) => Ok(None),
        Err(refusal) => Ok(Some(format!(
            "`migrate up` would refuse this database at the checkpoint `{}`: {refusal}",
            checkpoint_path.display()
        ))),
    }
}

/// Rewrites the recorded checksums and purges failed attempts.
pub(crate) fn repair_history<B: MigrationBackend>(
    backend: &mut B,
    migrations_dir: &Path,
    checkpoint_path: &Path,
) -> Result<CommandOutput, CliError> {
    // The tail is read inside the section the writers of the checkpoint artifacts
    // hold, and the section is released before the recorded history is touched: a
    // concurrent prune can neither delete a script between the listing and the
    // load of the same tail nor leave the repair observing a transient one.
    let migrations = {
        let _section = CheckpointLock::acquire(checkpoint_path)?;
        load_migrations_from_dir(migrations_dir).map_err(cli_error)?
    };
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
        return Err(CliError::message(message));
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
