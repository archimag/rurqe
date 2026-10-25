//! Parser of the `baseline` command.

use rurqe_migrate::version::MigrationVersion;

use super::{take_value, CliCommand, CliError};

/// Parses a `baseline` invocation into its typed command.
pub(super) fn parse_baseline(tokens: &[&str]) -> Result<CliCommand, CliError> {
    let mut version_str = None;
    let mut description = "baseline".to_owned();
    let mut database_url = None;

    let mut idx = 0;
    while idx < tokens.len() {
        let flag = tokens[idx];
        match flag {
            "--version" => version_str = Some(take_value(tokens, &mut idx, flag)?),
            "--description" => {
                description = take_value(tokens, &mut idx, flag)?.to_owned();
            }
            "--database-url" => {
                database_url = Some(take_value(tokens, &mut idx, flag)?.to_owned());
            }
            other => {
                return Err(CliError(format!("unknown option `{other}` for baseline")));
            }
        }
    }

    let version = match version_str {
        Some(v) => v
            .parse::<MigrationVersion>()
            .map_err(|e| CliError(format!("invalid baseline version `{v}`: {e}")))?,
        None => {
            return Err(CliError("missing required `--version` argument".to_owned()));
        }
    };

    Ok(CliCommand::Baseline {
        version,
        description,
        database_url,
    })
}
