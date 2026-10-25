//! Parser of the `prune` command.

use std::path::PathBuf;

use super::{
    default_checkpoint_path, default_migrations_dir, parse_version, take_value, CliCommand,
    CliError,
};

/// Parses `prune`: drop migration scripts the checkpoint already folded in.
pub(super) fn parse_prune(tokens: &[&str]) -> Result<CliCommand, CliError> {
    let mut before = None;
    let mut migrations_dir = default_migrations_dir();
    let mut checkpoint_path = default_checkpoint_path();
    let mut dry_run = false;

    let mut idx = 0;
    while idx < tokens.len() {
        let flag = tokens[idx];
        match flag {
            "--before" => {
                before = Some(parse_version(flag, take_value(tokens, &mut idx, flag)?)?);
            }
            "--migrations-dir" => {
                migrations_dir = PathBuf::from(take_value(tokens, &mut idx, flag)?);
            }
            "--checkpoint-path" => {
                checkpoint_path = PathBuf::from(take_value(tokens, &mut idx, flag)?);
            }
            "--dry-run" => {
                dry_run = true;
                idx += 1;
            }
            other => {
                return Err(CliError(format!("unknown option `{other}` for prune")));
            }
        }
    }

    let before = before
        .ok_or_else(|| CliError("missing required `--before` argument for prune".to_owned()))?;

    Ok(CliCommand::Prune {
        before,
        migrations_dir,
        checkpoint_path,
        dry_run,
    })
}
