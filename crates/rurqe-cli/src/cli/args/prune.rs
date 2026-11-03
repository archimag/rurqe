//! Parser of the `prune` command.

use super::{parse_version, take_path, take_value, universal_flag, CliCommand, CliError, Interrupt};

/// Parses `prune`: drop migration scripts the checkpoint already folded in.
pub(super) fn parse_prune(tokens: &[&str]) -> Result<CliCommand, Interrupt> {
    let mut config = None;
    let mut before = None;
    let mut migrations_dir = None;
    let mut checkpoint_path = None;
    let mut dry_run = false;

    let mut idx = 0;
    while idx < tokens.len() {
        let flag = tokens[idx];
        if let Some(command) = universal_flag(flag) {
            return Ok(command);
        }
        match flag {
            "--config" => config = Some(take_path(tokens, &mut idx, flag)?),
            "--before" => {
                before = Some(parse_version(flag, take_value(tokens, &mut idx, flag)?)?);
            }
            "--migrations-dir" => migrations_dir = Some(take_path(tokens, &mut idx, flag)?),
            "--checkpoint-path" => checkpoint_path = Some(take_path(tokens, &mut idx, flag)?),
            "--dry-run" => {
                dry_run = true;
                idx += 1;
            }
            other => {
                return Err(CliError::message(format!("unknown option `{other}` for prune")).into());
            }
        }
    }

    let before = before
        .ok_or_else(|| CliError::message("missing required `--before` argument for prune".to_owned()))?;

    Ok(CliCommand::Prune {
        config,
        before,
        migrations_dir,
        checkpoint_path,
        dry_run,
    })
}
