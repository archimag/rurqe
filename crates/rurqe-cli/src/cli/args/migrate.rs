//! Parser of the `migrate` command and its actions.

use super::{take_path, take_schema, take_value, universal_flag, CliCommand, CliError, Interrupt};

/// The action a `migrate` invocation asks for.
///
/// The action decides which inputs the invocation accepts, so it is resolved
/// before the flags are walked: `--checkpoint-path` arms the guard of `up`
/// alone, and an unknown action is reported as such whichever flags follow it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MigrateAction {
    Up,
    Status,
    Repair,
}

impl MigrateAction {
    /// Reads the action token, defaulting to `up` for a flag-led invocation.
    fn parse(token: &str) -> Result<Self, Interrupt> {
        match token {
            "up" => Ok(MigrateAction::Up),
            "status" => Ok(MigrateAction::Status),
            "repair" => Ok(MigrateAction::Repair),
            other => Err(CliError(format!(
                "unknown migrate action `{other}` (expected `up`, `status`, or `repair`)"
            ))
            .into()),
        }
    }

    /// The name the diagnostics of the action are written under.
    fn name(self) -> &'static str {
        match self {
            MigrateAction::Up => "up",
            MigrateAction::Status => "status",
            MigrateAction::Repair => "repair",
        }
    }
}

/// Parses a `migrate <action>` invocation into its typed command.
///
/// Without an action token the invocation defaults to `up`, so the cargo
/// entrypoint `cargo db-migrate` migrates when it receives nothing but flags.
pub(super) fn parse_migrate(tokens: &[&str]) -> Result<CliCommand, Interrupt> {
    let (action, tokens): (MigrateAction, &[&str]) = match tokens.split_first() {
        Some((action, tail)) if !action.starts_with('-') => (MigrateAction::parse(action)?, tail),
        _ => (MigrateAction::Up, tokens),
    };

    let mut config = None;
    let mut migrations_dir = None;
    let mut checkpoint_path = None;
    let mut database_url = None;
    let mut schema = None;

    let mut idx = 0;
    while idx < tokens.len() {
        let flag = tokens[idx];
        if let Some(command) = universal_flag(flag) {
            return Ok(command);
        }
        match flag {
            "--config" => config = Some(take_path(tokens, &mut idx, flag)?),
            "--migrations-dir" => migrations_dir = Some(take_path(tokens, &mut idx, flag)?),
            "--checkpoint-path" => {
                // The value is consumed before the action is consulted, so a
                // universal flag standing in its place is still answered and a
                // missing value keeps the diagnostic it carries everywhere.
                let value = take_path(tokens, &mut idx, flag)?;

                // The checkpoint arms the outdated-target guard of `up` alone,
                // so accepting the flag on the history actions would silently
                // discard an input the operator supplied on purpose.
                if action != MigrateAction::Up {
                    return Err(CliError(format!(
                        "unknown option `{flag}` for `migrate {}` (`--checkpoint-path` guards \
                         `migrate up` against a target older than the pruned boundary)",
                        action.name()
                    ))
                    .into());
                }

                checkpoint_path = Some(value);
            }
            "--database-url" => {
                database_url = Some(take_value(tokens, &mut idx, flag)?.to_owned());
            }
            "--schema" => {
                schema = Some(take_schema(tokens, &mut idx, flag)?);
            }
            other => {
                return Err(CliError(format!("unknown option `{other}` for migrate")).into());
            }
        }
    }

    match action {
        MigrateAction::Up => Ok(CliCommand::MigrateUp {
            config,
            migrations_dir,
            checkpoint_path,
            database_url,
            schema,
        }),
        MigrateAction::Status => Ok(CliCommand::MigrateStatus {
            config,
            migrations_dir,
            database_url,
            schema,
        }),
        MigrateAction::Repair => Ok(CliCommand::MigrateRepair {
            config,
            migrations_dir,
            database_url,
            schema,
        }),
    }
}
