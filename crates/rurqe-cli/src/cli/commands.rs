//! Behaviour of each CLI command: migrations, baseline, repair, codegen, and the
//! declarative stage plus checkpoint lifecycle.

use std::path::{Path, PathBuf};

use rurqe_migrate::checkpoint::Checkpoint;
use rurqe_migrate::migration::Migration;
use rurqe_migrate::runner::{MigrationBackend, MigrationStatusKind, Migrator};
use rurqe_migrate::stage::{cold_start, DeclarativeStage, StageMarker};
use rurqe_migrate::version::MigrationVersion;

use crate::codegen::CodegenOptions;
use crate::config::{CodegenConfig, DEFAULT_CONFIG_PATH};
use crate::introspect::introspect_database;
use crate::loader::load_migrations_from_dir;
use crate::stage::GoldenEquation;

use super::args::CliError;

/// Formats a displayable error as a [`CliError`].
fn cli_error(err: impl std::fmt::Display) -> CliError {
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
pub(super) fn initialise_stage<B: MigrationBackend>(
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
pub(super) fn verify_stage(
    stage_dir: &Path,
    base_stage_dir: Option<&Path>,
    migrations_dir: &Path,
    database_url: &str,
) -> Result<(), CliError> {
    let stage = load_stage(stage_dir)?;
    let base = base_stage_dir.map(load_stage).transpose()?;
    let tail = load_migrations_from_dir(migrations_dir).map_err(cli_error)?;

    let equation = match &base {
        Some(base) => GoldenEquation::new(&stage, &tail).with_base(base),
        None => GoldenEquation::new(&stage, &tail),
    };
    let report = equation.verify(database_url).map_err(cli_error)?;

    println!(
        "Golden equation holds: State({}) == State(Checkpoint + {}) (hash {})",
        stage_dir.display(),
        migrations_dir.display(),
        report.stage_hash()
    );
    Ok(())
}

/// Seals the live database as the new declarative stage anchor.
pub(super) fn commit_stage(
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

/// Prunes the migration scripts the committed checkpoint already folded in.
pub(super) fn prune_migrations(
    before: MigrationVersion,
    migrations_dir: &Path,
    checkpoint_path: &Path,
    dry_run: bool,
) -> Result<(), CliError> {
    let checkpoint = Checkpoint::load(checkpoint_path)
        .map_err(cli_error)?
        .ok_or_else(|| {
            CliError(format!(
                "`{}` is missing; commit the stage with `cargo rurqe stage commit --version {}` \
                 before pruning the history",
                checkpoint_path.display(),
                before
            ))
        })?;

    if checkpoint.pruned_up_to_version() < &before {
        return Err(CliError(format!(
            "`{}` anchors the history up to version {} only; pruning below {before} would drop \
             scripts the active tail still needs",
            checkpoint_path.display(),
            checkpoint.pruned_up_to_version()
        )));
    }

    let entries = match std::fs::read_dir(migrations_dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            println!(
                "No migration directory `{}` to prune",
                migrations_dir.display()
            );
            return Ok(());
        }
        Err(error) => {
            return Err(CliError(format!(
                "failed to read `{}`: {error}",
                migrations_dir.display()
            )))
        }
    };

    let mut pruned = Vec::new();

    for entry in entries {
        let path = entry
            .map_err(|error| {
                CliError(format!(
                    "failed to read `{}`: {error}",
                    migrations_dir.display()
                ))
            })?
            .path();

        if path.extension().and_then(|extension| extension.to_str()) != Some("sql") {
            continue;
        }

        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| CliError(format!("`{}` has no valid UTF-8 file name", path.display())))?;
        let migration = Migration::parse(name, "").map_err(cli_error)?;

        if migration.version.is_some_and(|version| version < before) {
            pruned.push(path);
        }
    }

    pruned.sort();

    if pruned.is_empty() {
        println!(
            "No migration below version {before} to prune in {}",
            migrations_dir.display()
        );
        return Ok(());
    }

    for path in &pruned {
        if dry_run {
            println!("would prune {}", path.display());
            continue;
        }
        std::fs::remove_file(path)
            .map_err(|error| CliError(format!("failed to remove `{}`: {error}", path.display())))?;
        println!("pruned {}", path.display());
    }

    println!(
        "{} {} migration(s) below version {before}",
        if dry_run { "Would prune" } else { "Pruned" },
        pruned.len()
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
    cargo rurqe stage init|verify|commit [options]
    cargo rurqe prune --before <version> [options]

COMMANDS:
    migrate up       Apply all pending migrations
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
    stage commit     --version <version> [--lock-path <path>] [--checkpoint-path <path>]
    prune            --before <version> [--migrations-dir <dir>] [--dry-run]

CONFIGURATION:
    `rurqe.toml` `[codegen.type_overrides]` maps a `table.column` onto the Rust
    type generated for it, e.g. `"users.id" = "uuid::Uuid"`.
"#
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use rurqe_migrate::{Checksum, InMemoryBackend, SchemaCatalog, STAGE_SCRIPT};

    fn workspace(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "rurqe-commands-{}-{name}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("create the workspace");
        path
    }

    fn version(text: &str) -> MigrationVersion {
        text.parse().expect("a valid version")
    }

    fn scripts(dir: &Path, names: &[&str]) {
        std::fs::create_dir_all(dir).expect("create the migration directory");
        for name in names {
            std::fs::write(dir.join(name), "SELECT 1;").expect("write the script");
        }
    }

    fn checkpoint_file(path: &Path, version: &str) {
        std::fs::write(
            path,
            Checkpoint::new(version.parse().expect("a valid version"), Checksum::of(b"stage"))
                .render(),
        )
        .expect("write the checkpoint");
    }

    #[test]
    fn the_stage_is_applied_from_the_declarative_directory() {
        let dir = workspace("stage-init");
        let stage_dir = dir.join("schema");
        std::fs::create_dir_all(&stage_dir).expect("create the stage directory");
        std::fs::write(
            stage_dir.join("users.sql"),
            "CREATE TABLE users (id INTEGER);",
        )
        .expect("write the stage script");

        let mut backend = InMemoryBackend::new();
        initialise_stage(&mut backend, &stage_dir, version("3.2"), "stage")
            .expect("the cold start succeeds");

        assert_eq!(backend.history.len(), 1);
        assert_eq!(backend.history[0].script, STAGE_SCRIPT);
        assert_eq!(backend.executed_sql.len(), 2);
    }

    #[test]
    fn committing_the_stage_anchors_the_checkpoint() {
        let dir = workspace("stage-commit");
        let lock_path = dir.join("schema.lock");
        let checkpoint_path = dir.join("checkpoint.toml");

        commit_stage(version("3.2"), ":memory:", &lock_path, &checkpoint_path)
            .expect("an empty database commits cleanly");

        let catalog = SchemaCatalog::from_lock(
            &std::fs::read_to_string(&lock_path).expect("read the lock"),
        )
        .expect("the lock is canonical");
        let checkpoint = Checkpoint::load(&checkpoint_path)
            .expect("the checkpoint is readable")
            .expect("the checkpoint exists");

        assert_eq!(checkpoint.pruned_up_to_version().to_string(), "3.2");
        assert_eq!(checkpoint.schema_hash(), catalog.checksum());
    }

    #[test]
    fn the_committed_boundary_is_never_rewound() {
        let dir = workspace("stage-rewind");
        let lock_path = dir.join("schema.lock");
        let checkpoint_path = dir.join("checkpoint.toml");
        commit_stage(version("5"), ":memory:", &lock_path, &checkpoint_path)
            .expect("the first commit succeeds");

        let error = commit_stage(version("3"), ":memory:", &lock_path, &checkpoint_path)
            .expect_err("the boundary cannot move backwards");

        assert!(error.to_string().contains("re-open"));
    }

    #[test]
    fn prune_removes_only_the_scripts_below_the_boundary() {
        let dir = workspace("prune");
        let migrations_dir = dir.join("migrations");
        scripts(
            &migrations_dir,
            &["V1__init.sql", "V2__orders.sql", "V9__tail.sql", "R__view.sql"],
        );
        let checkpoint_path = dir.join("checkpoint.toml");
        checkpoint_file(&checkpoint_path, "9");

        prune_migrations(version("3"), &migrations_dir, &checkpoint_path, false)
            .expect("the prune succeeds");

        let mut remaining: Vec<String> = std::fs::read_dir(&migrations_dir)
            .expect("read the migration directory")
            .map(|entry| {
                entry
                    .expect("a directory entry")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        remaining.sort();

        assert_eq!(remaining, vec!["R__view.sql", "V9__tail.sql"]);
    }

    #[test]
    fn prune_refuses_to_drop_scripts_the_tail_still_needs() {
        let dir = workspace("prune-boundary");
        let migrations_dir = dir.join("migrations");
        scripts(&migrations_dir, &["V2__orders.sql"]);
        let checkpoint_path = dir.join("checkpoint.toml");
        checkpoint_file(&checkpoint_path, "1");

        let error = prune_migrations(version("3"), &migrations_dir, &checkpoint_path, false)
            .expect_err("the checkpoint sits below the requested boundary");

        assert!(error.to_string().contains("still needs"));
        assert!(migrations_dir.join("V2__orders.sql").exists());
    }

    #[test]
    fn a_dry_run_keeps_every_script_in_place() {
        let dir = workspace("prune-dry-run");
        let migrations_dir = dir.join("migrations");
        scripts(&migrations_dir, &["V1__init.sql"]);
        let checkpoint_path = dir.join("checkpoint.toml");
        checkpoint_file(&checkpoint_path, "3");

        prune_migrations(version("3"), &migrations_dir, &checkpoint_path, true)
            .expect("the dry run succeeds");

        assert!(migrations_dir.join("V1__init.sql").exists());
    }
}
