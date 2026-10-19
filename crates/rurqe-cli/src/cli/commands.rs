//! Behaviour of each CLI command: migrations, baseline, repair, and codegen.

use std::path::{Path, PathBuf};

use rurqe_migrate::runner::{MigrationBackend, MigrationStatusKind, Migrator};
use rurqe_migrate::version::MigrationVersion;

use crate::loader::load_migrations_from_dir;

use super::args::CliError;

/// Formats a displayable error as a [`CliError`].
fn cli_error(err: impl std::fmt::Display) -> CliError {
    CliError(err.to_string())
}

/// Reads the generated schema code used for `SCHEMA_HASH` verification.
///
/// An explicitly supplied `--schema-path` must resolve to a readable file, so a
/// deleted or misconfigured schema file can never silently pass CI. When the
/// flag is omitted, the conventional `src/schema.rs` is used if present and
/// verification otherwise falls back to the lock snapshot alone with a warning.
pub(super) fn resolve_schema_code(explicit: Option<PathBuf>) -> Result<Option<String>, CliError> {
    let Some(path) = explicit else {
        let default = PathBuf::from("src/schema.rs");
        if !default.exists() {
            eprintln!(
                "warning: schema code `{}` not found; verifying against the lock snapshot only \
                 (pass `--schema-path` to require the generated code)",
                default.display()
            );
            return Ok(None);
        }
        return read_schema_code(&default).map(Some);
    };

    read_schema_code(&path).map(Some)
}

fn read_schema_code(path: &Path) -> Result<String, CliError> {
    std::fs::read_to_string(path).map_err(|e| {
        CliError(format!(
            "failed to read schema code `{}`: {e}",
            path.display()
        ))
    })
}

pub(super) fn write_file(path: &Path, contents: &str) -> Result<(), CliError> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(|e| {
                CliError(format!(
                    "failed to create directory `{}`: {e}",
                    parent.display()
                ))
            })?;
        }
    }
    std::fs::write(path, contents)
        .map_err(|e| CliError(format!("failed to write `{}`: {e}", path.display())))
}

pub(super) fn apply_migrations<B: MigrationBackend>(
    backend: &mut B,
    migrations_dir: &Path,
) -> Result<(), CliError> {
    let migrations = load_migrations_from_dir(migrations_dir).map_err(cli_error)?;
    let report = Migrator::new()
        .migrate(backend, &migrations)
        .map_err(cli_error)?;

    println!(
        "Applied {} migration(s) in {} ms",
        report.applied_count, report.total_time_ms
    );
    for script in &report.applied_scripts {
        println!("  - {script}");
    }
    Ok(())
}

pub(super) fn report_status<B: MigrationBackend>(
    backend: &mut B,
    migrations_dir: &Path,
) -> Result<(), CliError> {
    let migrations = load_migrations_from_dir(migrations_dir).map_err(cli_error)?;
    let statuses = Migrator::new()
        .status(backend, &migrations)
        .map_err(cli_error)?;

    if statuses.is_empty() {
        println!("No migrations found in {}", migrations_dir.display());
        return Ok(());
    }

    println!("{:<40} {:<12} {:<10} STATUS", "SCRIPT", "KIND", "VERSION");
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
        println!(
            "{:<40} {:<12} {:<10} {}",
            status.script,
            status.kind.as_str(),
            version,
            state
        );
    }
    Ok(())
}

pub(super) fn repair_history<B: MigrationBackend>(
    backend: &mut B,
    migrations_dir: &Path,
) -> Result<(), CliError> {
    let migrations = load_migrations_from_dir(migrations_dir).map_err(cli_error)?;
    let report = Migrator::new()
        .repair(backend, &migrations)
        .map_err(cli_error)?;

    println!(
        "Removed {} failed entr(ies), updated {} checksum(s)",
        report.removed_failed_entries,
        report.updated_checksums.len()
    );
    for script in &report.updated_checksums {
        println!("  - {script}");
    }
    Ok(())
}

pub(super) fn baseline_database<B: MigrationBackend>(
    backend: &mut B,
    version: MigrationVersion,
    description: &str,
) -> Result<(), CliError> {
    let entry = Migrator::new()
        .baseline(backend, version, description)
        .map_err(cli_error)?;
    println!(
        "Baselined database at rank {} ({})",
        entry.installed_rank, entry.description
    );
    Ok(())
}

pub(super) fn print_help() {
    println!(
        r#"RURQE Developer CLI

USAGE:
    cargo db-migrate [up|status|repair] [--migrations-dir <dir>]
    cargo db-baseline --version <version> [--description <desc>]
    cargo db-codegen [--output <path>] [--lock-path <path>]
    cargo db-verify [--schema-path <path>] [--lock-path <path>]

COMMANDS:
    migrate up       Apply all pending migrations
    migrate status   Inspect status of migrations against history table
    migrate repair   Fix checksums and purge failed migration attempts
    baseline         Mark current schema as baseline without running past migrations
    codegen          Introspect database, generate Rust schema, and write schema.lock
    verify           Verify bit-for-bit parity between live database and schema.lock
"#
    );
}
