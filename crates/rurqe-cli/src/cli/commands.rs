//! Behaviour of each CLI command, grouped by lifecycle area.
//!
//! `migrate` implements the Flyway-style history commands, `stage` the
//! declarative stage and checkpoint lifecycle, and `prune` the history pruning.
//! This facade keeps the helpers shared by those modules plus the help text and
//! re-exports the entry points consumed by the CLI dispatcher.

use std::path::{Path, PathBuf};

use crate::codegen::CodegenOptions;
use crate::config::{CodegenConfig, DEFAULT_CONFIG_PATH};

use super::args::CliError;

mod migrate;
mod prune;
mod stage;

#[cfg(test)]
mod tests;

pub(crate) use self::migrate::{apply_migrations, baseline_database, repair_history, report_status};
pub(crate) use self::prune::prune_migrations;
pub(crate) use self::stage::{commit_stage, initialise_stage, verify_stage};

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
    let type_overrides = config.into_type_overrides();

    Ok(CodegenOptions {
        type_overrides,
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
    std::fs::write(path, contents)
        .map_err(|e| CliError(format!("failed to write `{}`: {e}", path.display())))
}

pub(super) fn print_help() {
    println!(
        r#"RURQE Developer CLI

USAGE:
    cargo db-migrate up [--migrations-dir <dir>] <--checkpoint-path <path>|--no-checkpoint>
    cargo db-migrate [status|repair] [--migrations-dir <dir>]
    cargo db-baseline --version <version> [--description <desc>]
    cargo db-codegen [--output <path>] [--lock-path <path>]
    cargo db-verify [--schema-path <path>] [--lock-path <path>]
    cargo rurqe stage init|verify|commit [options]
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
    prune            Delete migration scripts below the committed checkpoint boundary

STAGE OPTIONS:
    stage init       --version <version> [--stage-dir <dir>] [--description <desc>]
    stage verify     [--stage-dir <dir>] [--base-stage-dir <dir>] [--migrations-dir <dir>]
                     [--scratch-url <url>]
    stage commit     --version <version> [--lock-path <path>] [--checkpoint-path <path>]
    prune            --before <version> [--migrations-dir <dir>] [--dry-run]

CONFIGURATION:
    `rurqe.toml` `[codegen.type_overrides]` maps a `table.column` onto the Rust
    type generated for it, e.g. `"users.id" = "uuid::Uuid"`.
"#
    );
}
