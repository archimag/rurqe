//! Parsers of the `codegen` and `verify` commands.

use std::path::PathBuf;

use super::{default_lock_path, default_schema_output, take_value, CliCommand, CliError};

/// Parses a `codegen` invocation into its typed command.
pub(super) fn parse_codegen(tokens: &[&str]) -> Result<CliCommand, CliError> {
    let mut database_url = None;
    let mut output = default_schema_output();
    let mut lock_path = default_lock_path();
    let mut domain_types = false;

    let mut idx = 0;
    while idx < tokens.len() {
        let flag = tokens[idx];
        match flag {
            "--database-url" => {
                database_url = Some(take_value(tokens, &mut idx, flag)?.to_owned());
            }
            "--output" | "-o" => {
                output = PathBuf::from(take_value(tokens, &mut idx, flag)?);
            }
            "--lock-path" => {
                lock_path = PathBuf::from(take_value(tokens, &mut idx, flag)?);
            }
            "--domain-types" => {
                domain_types = true;
                idx += 1;
            }
            other => {
                return Err(CliError(format!("unknown option `{other}` for codegen")));
            }
        }
    }

    Ok(CliCommand::Codegen {
        database_url,
        output,
        lock_path,
        domain_types,
    })
}

/// Parses a `verify` invocation into its typed command.
pub(super) fn parse_verify(tokens: &[&str]) -> Result<CliCommand, CliError> {
    let mut database_url = None;
    let mut schema_path = None;
    let mut lock_path = default_lock_path();

    let mut idx = 0;
    while idx < tokens.len() {
        let flag = tokens[idx];
        match flag {
            "--database-url" => {
                database_url = Some(take_value(tokens, &mut idx, flag)?.to_owned());
            }
            "--schema-path" => {
                schema_path = Some(PathBuf::from(take_value(tokens, &mut idx, flag)?));
            }
            "--lock-path" => {
                lock_path = PathBuf::from(take_value(tokens, &mut idx, flag)?);
            }
            other => {
                return Err(CliError(format!("unknown option `{other}` for verify")));
            }
        }
    }

    Ok(CliCommand::Verify {
        database_url,
        schema_path,
        lock_path,
    })
}
