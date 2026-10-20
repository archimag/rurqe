//! Pruning of the migration history already folded into the committed checkpoint.

use std::path::Path;

use rurqe_migrate::checkpoint::Checkpoint;
use rurqe_migrate::migration::Migration;
use rurqe_migrate::version::MigrationVersion;

use crate::cli::CliError;

use super::cli_error;

/// Prunes the migration scripts the committed checkpoint already folded in.
pub(crate) fn prune_migrations(
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
