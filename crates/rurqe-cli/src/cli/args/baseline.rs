//! Parser of the `baseline` command.

use rurqe_migrate::version::MigrationVersion;

use super::{take_path, take_value, universal_flag_beside_version, CliCommand, CliError, Interrupt};

/// Parses a `baseline` invocation into its typed command.
pub(super) fn parse_baseline(tokens: &[&str]) -> Result<CliCommand, Interrupt> {
    let mut config = None;
    let mut version_str = None;
    let mut description = "baseline".to_owned();
    let mut database_url = None;

    let mut idx = 0;
    while idx < tokens.len() {
        let flag = tokens[idx];
        if let Some(command) = universal_flag_beside_version(flag) {
            return Ok(command);
        }
        match flag {
            "--config" => config = Some(take_path(tokens, &mut idx, flag)?),
            "--version" => version_str = Some(take_value(tokens, &mut idx, flag)?),
            "--description" => {
                description = take_value(tokens, &mut idx, flag)?.to_owned();
            }
            "--database-url" => {
                database_url = Some(take_value(tokens, &mut idx, flag)?.to_owned());
            }
            other => {
                return Err(CliError::message(format!("unknown option `{other}` for baseline")).into());
            }
        }
    }

    let version = match version_str {
        Some(v) => v
            .parse::<MigrationVersion>()
            .map_err(|e| CliError::message(format!("invalid baseline version `{v}`: {e}")))?,
        None => {
            return Err(CliError::message("missing required `--version` argument").into());
        }
    };

    Ok(CliCommand::Baseline {
        config,
        version,
        description,
        database_url,
    })
}
