//! Pruning of the migration history already folded into the committed checkpoint.

use std::path::Path;

use rurqe_migrate::checkpoint::Checkpoint;
use rurqe_migrate::migration::Migration;
use rurqe_migrate::scripts::sql_scripts;
use rurqe_migrate::version::MigrationVersion;

use crate::cli::{CliError, CommandOutput};

use super::cli_error;

/// Deletes the migration scripts the committed checkpoint already folded in.
pub(crate) fn prune_migrations(
    before: MigrationVersion,
    migrations_dir: &Path,
    checkpoint_path: &Path,
    dry_run: bool,
) -> Result<CommandOutput, CliError> {
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

    let scripts = match sql_scripts(migrations_dir) {
        Ok(Some(scripts)) => scripts,
        Ok(None) => {
            return Ok(CommandOutput::single(format!(
                "No migration directory `{}` to prune",
                migrations_dir.display()
            )))
        }
        Err(error) => return Err(cli_error(error)),
    };

    let mut pruned = Vec::new();

    for script in scripts {
        let migration = Migration::parse(script.name(), "").map_err(cli_error)?;

        if migration.version.is_some_and(|version| version < before) {
            pruned.push(script.path().to_owned());
        }
    }

    pruned.sort();

    if pruned.is_empty() {
        return Ok(CommandOutput::single(format!(
            "No migration below version {before} to prune in {}",
            migrations_dir.display()
        )));
    }

    let mut output = CommandOutput::new();

    for path in &pruned {
        if dry_run {
            output.line(format!("would prune {}", path.display()));
            continue;
        }
        std::fs::remove_file(path)
            .map_err(|error| CliError(format!("failed to remove `{}`: {error}", path.display())))?;
        output.line(format!("pruned {}", path.display()));
    }

    output.line(format!(
        "{} {} migration(s) below version {before}",
        if dry_run { "Would prune" } else { "Pruned" },
        pruned.len()
    ));
    Ok(output)
}
