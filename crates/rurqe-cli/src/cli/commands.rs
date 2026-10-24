//! Behaviour of each CLI command, grouped by lifecycle area.
//!
//! `migrate` implements the Flyway-style history commands, `stage` the
//! declarative stage, checkpoint, and scratch lifecycle, and `prune` the history
//! pruning.
//! This facade keeps the helpers shared by those modules plus the help text and
//! re-exports the entry points consumed by the CLI dispatcher.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::codegen::CodegenOptions;
use crate::config::{CodegenConfig, DEFAULT_CONFIG_PATH};

use super::args::CliError;

mod migrate;
mod prune;
mod stage;

#[cfg(test)]
mod tests;

pub(crate) use self::migrate::{
    apply_migrations, baseline_database, repair_history, report_status,
};
pub(crate) use self::prune::prune_migrations;
pub(crate) use self::stage::{cleanup_stage, commit_stage, initialise_stage, verify_stage};

/// Formats a displayable error as a [`CliError`].
pub(super) fn cli_error(err: impl std::fmt::Display) -> CliError {
    CliError(err.to_string())
}

/// Loads the codegen options of the project from `rurqe.toml`.
///
/// The file is optional: a project without it generates the engine-neutral
/// types, while a malformed file aborts generation instead of silently emitting
/// wrong types for overridden columns.
pub(super) fn codegen_options() -> Result<CodegenOptions, CliError> {
    let config = CodegenConfig::load(Path::new(DEFAULT_CONFIG_PATH)).map_err(cli_error)?;
    let domain_types = config.domain_types();
    let type_overrides = config.into_type_overrides();

    Ok(CodegenOptions {
        type_overrides,
        domain_types,
        ..CodegenOptions::default()
    })
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

    // The bytes are staged next to the artifact and renamed into place, so a
    // failure while they are written leaves the previous file untouched instead
    // of truncating it. A lock and the checkpoint derived from it therefore never
    // disagree about the snapshot they describe.
    let temporary = temporary_sibling(path);
    write_temporary(&temporary, contents, path)?;

    std::fs::rename(&temporary, path).map_err(|error| {
        let _ = std::fs::remove_file(&temporary);
        CliError(format!("failed to write `{}`: {error}", path.display()))
    })
}

/// The temporary file an artifact is staged in before it is renamed.
///
/// The name stays a hidden sibling of the artifact, and the process id together
/// with a monotonic counter keeps two writers of one path apart.
fn temporary_sibling(path: &Path) -> PathBuf {
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);

    let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let temporary = format!(".{name}.{}.{sequence}.tmp", std::process::id());

    match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.join(temporary),
        _ => PathBuf::from(temporary),
    }
}

/// Writes the bytes of an artifact and flushes them to the device.
fn write_temporary(temporary: &Path, contents: &str, path: &Path) -> Result<(), CliError> {
    use std::io::Write;

    let mut file =
        std::fs::File::create(temporary).map_err(|error| write_failure(temporary, path, &error))?;
    file.write_all(contents.as_bytes())
        .map_err(|error| write_failure(temporary, path, &error))?;
    file.sync_all()
        .map_err(|error| write_failure(temporary, path, &error))?;

    Ok(())
}

/// Removes the staged file a failed write left behind and names the artifact.
fn write_failure(temporary: &Path, path: &Path, error: &std::io::Error) -> CliError {
    let _ = std::fs::remove_file(temporary);
    CliError(format!("failed to write `{}`: {error}", path.display()))
}

pub(super) fn print_help() {
    println!(
        r#"RURQE Developer CLI

USAGE:
    cargo db-migrate up [--migrations-dir <dir>] [--checkpoint-path <path>] [--database-url <url>]
    cargo db-migrate [status|repair] [--migrations-dir <dir>]
    cargo db-baseline --version <version> [--description <desc>]
    cargo db-codegen [--output <path>] [--lock-path <path>] [--domain-types]
    cargo db-verify [--schema-path <path>] [--lock-path <path>]
    cargo rurqe stage init|verify|commit|cleanup [options]
    cargo rurqe prune --before <version> [options]

COMMANDS:
    migrate up       Apply all pending migrations, guarded by `checkpoint.toml`
    migrate status   Inspect status of migrations against history table
    migrate repair   Fix checksums and purge failed migration attempts
    baseline         Mark current schema as baseline without running past migrations
    codegen          Introspect database, generate Rust schema, and write schema.lock
    verify           Verify bit-for-bit parity between live database and schema.lock
    stage init       Cold start the database from the declarative stage (`schema/`)
    stage verify     Verify the golden equation State(schema/) == State(Checkpoint + tail)
    stage commit     Seal the live database as `schema.lock` and anchor `checkpoint.toml`
    stage cleanup    Drop the PostgreSQL scratch databases abandoned by earlier verifications
    prune            Delete migration scripts below the committed checkpoint boundary

STAGE OPTIONS:
    stage init       --version <version> [--stage-dir <dir>] [--description <desc>]
    stage verify     [--stage-dir <dir>] [--base-stage-dir <dir>] [--migrations-dir <dir>]
                     [--checkpoint-path <path>] [--scratch-url <url>]
    stage commit     --version <version> [--lock-path <path>] [--checkpoint-path <path>]
    stage cleanup    [--scratch-url <url>] [--older-than <secs>] [--yes|--force]
    prune            --before <version> [--migrations-dir <dir>] [--dry-run]

CONFIGURATION:
    `rurqe.toml` `[codegen.type_overrides]` maps a `table.column` onto the Rust
    type generated for it, e.g. `"users.id" = "uuid::Uuid"`.
"#
    );
}
