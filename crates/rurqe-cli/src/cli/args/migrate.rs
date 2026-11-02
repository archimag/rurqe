//! Parser of the `migrate` command and its actions.

use super::{take_path, take_schema, take_value, universal_flag, CliCommand, CliError, Interrupt};

/// The action a `migrate` invocation asks for.
///
/// The action decides which command the invocation produces, so it is resolved
/// before the flags are walked: `up` applies the active tail over the boundary
/// marker, `status` and `repair` read and rewrite the recorded history, and
/// `clean` erases the whole target — the recorded history included — without
/// applying anything. An unknown action is reported as such whichever flags
/// follow it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MigrateAction {
    Up,
    Status,
    Repair,
    Clean,
}

impl MigrateAction {
    /// Reads the action token, defaulting to `up` for a flag-led invocation.
    fn parse(token: &str) -> Result<Self, Interrupt> {
        match token {
            "up" => Ok(MigrateAction::Up),
            "status" => Ok(MigrateAction::Status),
            "repair" => Ok(MigrateAction::Repair),
            "clean" => Ok(MigrateAction::Clean),
            other => Err(CliError(format!(
                "unknown migrate action `{other}` (expected `up`, `status`, `repair`, or `clean`)"
            ))
            .into()),
        }
    }
}

/// Parses a `migrate <action>` invocation into its typed command.
///
/// Without an action token the invocation defaults to `up`, so the cargo
/// entrypoint `cargo db-migrate` migrates when it receives nothing but flags.
///
/// The destructive `clean` action additionally takes the `--yes` (`-y`,
/// `--force`) confirmation the command gates on, while every other action
/// refuses it: they advance or rewrite the recorded history instead of erasing
/// the database that holds it.
///
/// The family takes no path option of its own: the migration tail and the
/// boundary marker are resolved from the project configuration, so the only
/// inputs an invocation supplies are the configuration it reads, the database it
/// opens, and the schema its catalog digests describe. A path option of the
/// retired cascade is therefore reported instead of being accepted and ignored.
pub(super) fn parse_migrate(tokens: &[&str]) -> Result<CliCommand, Interrupt> {
    let (action, tokens): (MigrateAction, &[&str]) = match tokens.split_first() {
        Some((action, tail)) if !action.starts_with('-') => (MigrateAction::parse(action)?, tail),
        _ => (MigrateAction::Up, tokens),
    };

    let mut config = None;
    let mut database_url = None;
    let mut schema = None;
    let mut force = false;

    let mut idx = 0;
    while idx < tokens.len() {
        let flag = tokens[idx];
        if let Some(command) = universal_flag(flag) {
            return Ok(command);
        }
        match flag {
            "--config" => config = Some(take_path(tokens, &mut idx, flag)?),
            "--database-url" => {
                database_url = Some(take_value(tokens, &mut idx, flag)?.to_owned());
            }
            "--schema" => {
                schema = Some(take_schema(tokens, &mut idx, flag)?);
            }
            "--yes" | "-y" | "--force" => {
                if action != MigrateAction::Clean {
                    return Err(CliError(format!(
                        "option `{flag}` confirms `migrate clean`; the `up`, `status`, and \
                         `repair` actions take no confirmation"
                    ))
                    .into());
                }
                force = true;
                idx += 1;
            }
            other => {
                return Err(CliError(format!("unknown option `{other}` for migrate")).into());
            }
        }
    }

    match action {
        MigrateAction::Up => Ok(CliCommand::MigrateUp {
            config,
            database_url,
            schema,
        }),
        MigrateAction::Status => Ok(CliCommand::MigrateStatus {
            config,
            database_url,
            schema,
        }),
        MigrateAction::Repair => Ok(CliCommand::MigrateRepair {
            config,
            database_url,
            schema,
        }),
        MigrateAction::Clean => Ok(CliCommand::MigrateClean {
            config,
            database_url,
            schema,
            force,
        }),
    }
}
