//! Parsers of the `codegen` and `verify` commands.

use super::{take_path, take_schema, take_value, universal_flag, CliCommand, CliError, Interrupt};

/// Parses a `codegen` invocation into its typed command.
pub(super) fn parse_codegen(tokens: &[&str]) -> Result<CliCommand, Interrupt> {
    let mut config = None;
    let mut database_url = None;
    let mut output = None;
    let mut lock_path = None;
    let mut domain_types = false;
    let mut schema = None;

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
            "--output" | "-o" => output = Some(take_path(tokens, &mut idx, flag)?),
            "--lock-path" => lock_path = Some(take_path(tokens, &mut idx, flag)?),
            "--domain-types" => {
                domain_types = true;
                idx += 1;
            }
            "--schema" => {
                schema = Some(take_schema(tokens, &mut idx, flag)?);
            }
            other => {
                return Err(
                    CliError::message(format!("unknown option `{other}` for codegen")).into(),
                );
            }
        }
    }

    Ok(CliCommand::Codegen {
        config,
        database_url,
        output,
        lock_path,
        domain_types,
        schema,
    })
}

/// Parses a `verify` invocation into its typed command.
pub(super) fn parse_verify(tokens: &[&str]) -> Result<CliCommand, Interrupt> {
    let mut config = None;
    let mut database_url = None;
    let mut schema_path = None;
    let mut lock_path = None;
    let mut schema = None;

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
            "--schema-path" => schema_path = Some(take_path(tokens, &mut idx, flag)?),
            "--lock-path" => lock_path = Some(take_path(tokens, &mut idx, flag)?),
            "--schema" => {
                schema = Some(take_schema(tokens, &mut idx, flag)?);
            }
            other => {
                return Err(
                    CliError::message(format!("unknown option `{other}` for verify")).into(),
                );
            }
        }
    }

    Ok(CliCommand::Verify {
        config,
        database_url,
        schema_path,
        lock_path,
        schema,
    })
}
