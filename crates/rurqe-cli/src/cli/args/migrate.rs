//! Parser of the `migrate` command and its actions.

use std::path::PathBuf;

use super::{
    default_checkpoint_path, default_migrations_dir, take_schema, take_value, universal_flag,
    CliCommand, CliError, Interrupt,
};

/// Parses a `migrate <action>` invocation into its typed command.
///
/// Without an action token the invocation defaults to `up`, so the cargo
/// entrypoint `cargo db-migrate` migrates when it receives nothing but flags.
pub(super) fn parse_migrate(tokens: &[&str]) -> Result<CliCommand, Interrupt> {
    let (action, tokens): (&str, &[&str]) = match tokens.split_first() {
        Some((action, tail)) if !action.starts_with('-') => (*action, tail),
        _ => ("up", tokens),
    };

    let mut migrations_dir = default_migrations_dir();
    let mut checkpoint_path = default_checkpoint_path();
    let mut database_url = None;
    let mut schema = None;

    let mut idx = 0;
    while idx < tokens.len() {
        let flag = tokens[idx];
        if let Some(command) = universal_flag(flag) {
            return Ok(command);
        }
        match flag {
            "--migrations-dir" => {
                migrations_dir = PathBuf::from(take_value(tokens, &mut idx, flag)?);
            }
            "--checkpoint-path" => {
                checkpoint_path = PathBuf::from(take_value(tokens, &mut idx, flag)?);
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
        "up" => Ok(CliCommand::MigrateUp {
            migrations_dir,
            checkpoint_path,
            database_url,
            schema,
        }),
        "status" => Ok(CliCommand::MigrateStatus {
            migrations_dir,
            database_url,
            schema,
        }),
        "repair" => Ok(CliCommand::MigrateRepair {
            migrations_dir,
            database_url,
            schema,
        }),
        other => Err(CliError(format!(
            "unknown migrate action `{other}` (expected `up`, `status`, or `repair`)"
        ))
        .into()),
    }
}
