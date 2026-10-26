//! Behaviour of each CLI command, grouped by lifecycle area.
//!
//! `migrate` implements the Flyway-style history commands, `stage` the
//! declarative stage, checkpoint, and scratch lifecycle, and `prune` the history
//! pruning.
//! This facade keeps the helpers shared by those modules plus the help text and
//! re-exports the entry points consumed by the CLI dispatcher.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use crate::codegen::CodegenOptions;
use crate::config::CodegenConfig;
use crate::introspect::DEFAULT_SCHEMA;

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

/// Loads the project configuration from the file at `path`.
///
/// The path comes from the caller instead of being read from the working
/// directory here, so a command resolves the configuration of the project it was
/// pointed at. The file is optional: a project without it keeps the engine-neutral
/// types and the default schema, while a malformed file aborts the command instead
/// of silently emitting wrong types for overridden columns or describing the wrong
/// schema.
pub(super) fn load_config(path: &Path) -> Result<CodegenConfig, CliError> {
    CodegenConfig::load(path).map_err(cli_error)
}

/// Resolves the schema a command's catalog digests describe.
///
/// The `--schema` argument wins over the `schema` key of the configuration file at
/// `config_path`, and the libpq default `public` applies when neither names a
/// schema. The single resolved value reaches the codegen snapshot, the drift gate,
/// the cold-start marker, and the checkpoint guard, so those digests always
/// describe the same schema.
pub(super) fn resolve_schema(
    explicit: Option<&str>,
    config_path: &Path,
) -> Result<String, CliError> {
    Ok(schema_of(explicit, &load_config(config_path)?))
}

/// Picks the schema out of an already loaded configuration.
pub(super) fn schema_of(explicit: Option<&str>, config: &CodegenConfig) -> String {
    explicit
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .or_else(|| config.schema())
        .unwrap_or(DEFAULT_SCHEMA)
        .to_owned()
}

/// Reads the code generation options out of the project configuration.
pub(super) fn codegen_options(config: CodegenConfig) -> CodegenOptions {
    let domain_types = config.domain_types();
    let type_overrides = config.into_type_overrides();

    CodegenOptions {
        type_overrides,
        domain_types,
        ..CodegenOptions::default()
    }
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
            tracing::warn!(
                schema_code = %default.display(),
                "schema code not found; verifying against the lock snapshot only \
                 (pass `--schema-path` to require the generated code)"
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
    // of truncating it. The publication is completed by the flush below, so a
    // crash can no longer lose the rename and resurrect the previous
    // `schema.lock` or `checkpoint.toml` while the new bytes sit durable in an
    // orphaned staging file. A lock and the checkpoint derived from it
    // therefore never disagree about the snapshot they describe.
    let temporary = temporary_sibling(path);
    write_temporary(&temporary, contents, path)?;
    rename_durable(&temporary, path)
}

/// Renames a staged artifact into place and flushes the directory entry.
///
/// The rename a staged write publishes its bytes with becomes durable only
/// once the directory holding the artifact is flushed, so the flush is part
/// of the write instead of an afterthought: a flush that cannot be completed
/// fails the write rather than leaving a marker a crash can silently swap
/// back for the boundary beside it.
fn rename_durable(temporary: &Path, path: &Path) -> Result<(), CliError> {
    std::fs::rename(temporary, path).map_err(|error| {
        let _ = std::fs::remove_file(temporary);
        CliError(format!("failed to write `{}`: {error}", path.display()))
    })?;

    sync_directory(artifact_directory(path)).map_err(|error| {
        CliError(format!(
            "failed to flush the directory holding `{}` after writing it: {error}",
            path.display()
        ))
    })
}

/// The suffix every artifact is staged under before it is renamed into place.
const TEMPORARY_SUFFIX: &str = ".tmp";

/// The age above which a staged artifact counts as abandoned.
///
/// A staged artifact only lives for the duration of one file write or one
/// directory swap, so anything this old belongs to a process that was killed
/// before it could rename or remove it.
pub(super) const STALE_TEMPORARY_AGE: Duration = Duration::from_secs(24 * 60 * 60);

/// The directory an artifact and the staged siblings it is written through live in.
pub(super) fn artifact_directory(path: &Path) -> &Path {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

/// The prefix every staged sibling of `artifact` starts with.
fn temporary_prefix(artifact: &Path) -> String {
    let name = artifact
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();

    format!(".{name}.")
}

/// The process id embedded in the name of a staged sibling of `prefix`.
///
/// A staged sibling is named `.<basename>.<pid>.<sequence>.tmp`, so its owner is
/// the segment right after the prefix once the suffix is stripped.
fn staged_owner(name: &str, prefix: &str) -> Option<u32> {
    let rest = name.strip_prefix(prefix)?.strip_suffix(TEMPORARY_SUFFIX)?;
    let (owner, _) = rest.rsplit_once('.')?;

    owner.parse().ok()
}

/// The temporary file an artifact is staged in before it is renamed.
///
/// The name stays a hidden sibling of the artifact, and the process id together
/// with a monotonic counter keeps two writers of one path apart.
fn temporary_sibling(path: &Path) -> PathBuf {
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);

    let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);

    artifact_directory(path).join(format!(
        "{}{}.{sequence}{TEMPORARY_SUFFIX}",
        temporary_prefix(path),
        std::process::id()
    ))
}

/// Removes the staged siblings an earlier process abandoned next to `artifact`.
///
/// Every artifact is written through a hidden sibling named
/// `.<basename>.<pid>.<sequence>.tmp` and renamed into place, so a process killed
/// in between strands that sibling for good: the next run derives a fresh
/// sequence and never touches the old one again. The debris is swept here —
/// every sibling last touched longer than `older_than` ago is discarded, while a
/// name this process could still own is kept, so a sweep never deletes a staging
/// path another call of this process is using right now.
///
/// A directory that cannot be scanned is reported, while a single sibling that
/// cannot be removed only leaves the artifact it was staged for untouched: the
/// caller works on the real artifact either way.
pub(super) fn sweep_stale_temporaries(
    artifact: &Path,
    older_than: Duration,
) -> Result<Vec<PathBuf>, CliError> {
    let directory = artifact_directory(artifact);
    let prefix = temporary_prefix(artifact);
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(CliError(format!(
                "failed to scan `{}` for staged artifacts: {error}",
                directory.display()
            )))
        }
    };

    let mut removed = Vec::new();

    for entry in entries {
        let entry = entry.map_err(|error| {
            CliError(format!(
                "failed to scan `{}` for staged artifacts: {error}",
                directory.display()
            ))
        })?;
        let name = entry.file_name().to_string_lossy().into_owned();

        // A sibling this process could still own is kept: its writer is alive and
        // may rename it into place at any moment.
        if !matches!(
            staged_owner(&name, &prefix),
            Some(owner) if owner != std::process::id()
        ) {
            continue;
        }

        if let Some(path) = discard_if_abandoned(&entry, older_than) {
            removed.push(path);
        }
    }

    Ok(removed)
}

/// Discards `entry` when an interrupted write abandoned it, returning its path.
///
/// A sibling that cannot be dated is kept, because a sweep that guessed would
/// delete a staging path a live writer is still filling.
fn discard_if_abandoned(entry: &std::fs::DirEntry, older_than: Duration) -> Option<PathBuf> {
    let metadata = entry.metadata().ok()?;
    let modified = metadata.modified().ok()?;
    let elapsed = std::time::SystemTime::now().duration_since(modified).ok()?;

    if elapsed < older_than {
        return None;
    }

    let path = entry.path();
    let removal = if metadata.is_dir() {
        std::fs::remove_dir_all(&path)
    } else {
        std::fs::remove_file(&path)
    };

    match removal {
        Ok(()) => Some(path),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Some(path),
        Err(error) => {
            tracing::warn!(
                artifact = %path.display(),
                error = %error,
                "failed to discard a staged artifact an interrupted write left behind"
            );
            None
        }
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

/// Flushes the entries of `dir` to the device.
///
/// A rename publishes a new name while keeping the staged contents, so an
/// artifact written or swapped through a staging sibling is durable only once
/// the directory holding its entry is flushed. `write_file` fails when the
/// flush cannot be completed, while the boundary swap reports it and keeps the
/// snapshot it already installed, so both policies share this one primitive
/// and the discipline cannot drift between call sites. Only Unix exposes a
/// directory as a file handle; a platform without that capability leaves the
/// ordering to the filesystem instead of reporting every write as a failure it
/// cannot do anything about.
#[cfg(unix)]
pub(super) fn sync_directory(dir: &Path) -> Result<(), CliError> {
    let handle = std::fs::File::open(dir).map_err(|error| {
        CliError(format!(
            "failed to open the directory `{}`: {error}",
            dir.display()
        ))
    })?;

    handle.sync_all().map_err(|error| {
        CliError(format!(
            "failed to flush the directory `{}`: {error}",
            dir.display()
        ))
    })
}

#[cfg(not(unix))]
pub(super) fn sync_directory(_dir: &Path) -> Result<(), CliError> {
    Ok(())
}

/// The help text of the toolchain.
///
/// The text is returned instead of printed, so the process entry point decides
/// where the help of the toolchain goes.
pub(super) fn help_text() -> &'static str {
    r#"RURQE Developer CLI

USAGE:
    cargo db-migrate up [--migrations-dir <dir>] [--checkpoint-path <path>] [--database-url <url>]
                       [--schema <name>]
    cargo db-migrate [status|repair] [--migrations-dir <dir>] [--schema <name>]
    cargo db-baseline --version <version> [--description <desc>]
    cargo db-codegen [--output <path>] [--lock-path <path>] [--domain-types] [--schema <name>]
    cargo db-verify [--schema-path <path>] [--lock-path <path>] [--schema <name>]
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
    stage commit     Seal the live database as `schema.lock`, snapshot the boundary stage,
                     and anchor `checkpoint.toml`
    stage cleanup    Drop the PostgreSQL scratch databases abandoned by earlier verifications
    prune            Delete migration scripts below the committed checkpoint boundary

STAGE OPTIONS:
    stage init       --version <version> [--stage-dir <dir>] [--description <desc>]
    stage verify     [--stage-dir <dir>] [--base-stage-dir <dir>] [--migrations-dir <dir>]
                     [--checkpoint-path <path>] [--scratch-url <url>]
    stage commit     --version <version> [--stage-dir <dir>] [--lock-path <path>]
                     [--checkpoint-path <path>]
    stage cleanup    [--scratch-url <url>] [--older-than <secs>] [--yes|--force]
    prune            --before <version> [--migrations-dir <dir>] [--dry-run]

NOTES:
    `stage commit` snapshots the declarative stage verbatim into `checkpoint-stage/`
    next to `checkpoint.toml`. `stage verify` loads that snapshot as the base stage
    when `--base-stage-dir` is omitted, and a checkpoint whose non-empty boundary has
    no base stage is rejected instead of replaying the tail over an empty database.

SCHEMA SELECTION:
    --schema <name>  PostgreSQL schema every catalog digest describes. A command line
                     selection wins over the `schema` key of `rurqe.toml`, which wins
                     over `public`. A selected schema that holds no table while the
                     database keeps user relations elsewhere is reported instead of
                     anchoring an empty catalog hash.

CONFIGURATION:
    `rurqe.toml` `schema = "app"` names the described schema, and
    `[codegen.type_overrides]` maps a `table.column` onto the Rust type generated
    for it, e.g. `"users.id" = "uuid::Uuid"`.
"#
}
