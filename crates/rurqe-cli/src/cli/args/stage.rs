//! Parsers of the `stage` command and its actions.

use std::path::PathBuf;

use super::{
    default_checkpoint_path, default_lock_path, default_migrations_dir, default_stage_dir,
    parse_version, take_schema, take_value, universal_flag, universal_flag_beside_version,
    CliCommand, CliError,
};

/// The default age above which a scratch database counts as abandoned.
const DEFAULT_CLEANUP_AGE: u64 = crate::stage::postgres::DEFAULT_SCRATCH_MAX_AGE.as_secs();

/// Parses a `stage <action>` invocation into its typed command.
pub(super) fn parse_stage(tokens: &[&str]) -> Result<CliCommand, CliError> {
    let action = match tokens.first() {
        Some(action) if !action.starts_with('-') => *action,
        other => {
            // A universal flag standing where the action belongs is answered
            // before the action is looked up, so `rurqe stage --help` prints the
            // help instead of demanding a stage action.
            if let Some(command) = other.copied().and_then(universal_flag) {
                return Ok(command);
            }
            return Err(CliError(
                "missing stage action (expected `init`, `verify`, `commit`, or `cleanup`)"
                    .to_owned(),
            ));
        }
    };

    match action {
        "init" => parse_stage_init(&tokens[1..]),
        "verify" => parse_stage_verify(&tokens[1..]),
        "commit" => parse_stage_commit(&tokens[1..]),
        "cleanup" => parse_stage_cleanup(&tokens[1..]),
        other => Err(CliError(format!(
            "unknown stage action `{other}` (expected `init`, `verify`, `commit`, or `cleanup`)"
        ))),
    }
}

/// Parses `stage init`: cold start the database from the declarative stage.
fn parse_stage_init(tokens: &[&str]) -> Result<CliCommand, CliError> {
    let mut stage_dir = default_stage_dir();
    let mut description = "stage".to_owned();
    let mut version = None;
    let mut database_url = None;
    let mut schema = None;

    let mut idx = 0;
    while idx < tokens.len() {
        let flag = tokens[idx];
        if let Some(command) = universal_flag_beside_version(flag) {
            return Ok(command);
        }
        match flag {
            "--stage-dir" => {
                stage_dir = PathBuf::from(take_value(tokens, &mut idx, flag)?);
            }
            "--description" => {
                description = take_value(tokens, &mut idx, flag)?.to_owned();
            }
            "--version" => {
                version = Some(parse_version(flag, take_value(tokens, &mut idx, flag)?)?);
            }
            "--database-url" => {
                database_url = Some(take_value(tokens, &mut idx, flag)?.to_owned());
            }
            "--schema" => {
                schema = Some(take_schema(tokens, &mut idx, flag)?);
            }
            other => {
                return Err(CliError(format!("unknown option `{other}` for stage init")));
            }
        }
    }

    let version = version.ok_or_else(|| {
        CliError("missing required `--version` argument for stage init".to_owned())
    })?;

    Ok(CliCommand::StageInit {
        stage_dir,
        version,
        description,
        database_url,
        schema,
    })
}

/// Parses `stage verify`: check the golden equation between stage and tail.
fn parse_stage_verify(tokens: &[&str]) -> Result<CliCommand, CliError> {
    let mut stage_dir = default_stage_dir();
    let mut base_stage_dir = None;
    let mut migrations_dir = default_migrations_dir();
    let mut checkpoint_path = default_checkpoint_path();
    let mut scratch_url = None;
    let mut database_url = None;

    let mut idx = 0;
    while idx < tokens.len() {
        let flag = tokens[idx];
        if let Some(command) = universal_flag(flag) {
            return Ok(command);
        }
        match flag {
            "--stage-dir" => {
                stage_dir = PathBuf::from(take_value(tokens, &mut idx, flag)?);
            }
            "--base-stage-dir" => {
                base_stage_dir = Some(PathBuf::from(take_value(tokens, &mut idx, flag)?));
            }
            "--scratch-url" => {
                scratch_url = Some(take_value(tokens, &mut idx, flag)?.to_owned());
            }
            "--migrations-dir" => {
                migrations_dir = PathBuf::from(take_value(tokens, &mut idx, flag)?);
            }
            "--checkpoint-path" => {
                checkpoint_path = PathBuf::from(take_value(tokens, &mut idx, flag)?);
            }
            "--database-url" => {
                database_url = Some(take_value(tokens, &mut idx, flag)?.to_owned());
            }
            other => {
                return Err(CliError(format!(
                    "unknown option `{other}` for stage verify"
                )));
            }
        }
    }

    Ok(CliCommand::StageVerify {
        stage_dir,
        base_stage_dir,
        migrations_dir,
        checkpoint_path,
        scratch_url,
        database_url,
    })
}

/// Parses `stage commit`: seal the live database as the new stage anchor.
fn parse_stage_commit(tokens: &[&str]) -> Result<CliCommand, CliError> {
    let mut version = None;
    let mut stage_dir = default_stage_dir();
    let mut lock_path = default_lock_path();
    let mut checkpoint_path = default_checkpoint_path();
    let mut database_url = None;
    let mut schema = None;

    let mut idx = 0;
    while idx < tokens.len() {
        let flag = tokens[idx];
        if let Some(command) = universal_flag_beside_version(flag) {
            return Ok(command);
        }
        match flag {
            "--version" => {
                version = Some(parse_version(flag, take_value(tokens, &mut idx, flag)?)?);
            }
            "--lock-path" => {
                lock_path = PathBuf::from(take_value(tokens, &mut idx, flag)?);
            }
            "--checkpoint-path" => {
                checkpoint_path = PathBuf::from(take_value(tokens, &mut idx, flag)?);
            }
            "--database-url" => {
                database_url = Some(take_value(tokens, &mut idx, flag)?.to_owned());
            }
            "--stage-dir" => {
                stage_dir = PathBuf::from(take_value(tokens, &mut idx, flag)?);
            }
            "--schema" => {
                schema = Some(take_schema(tokens, &mut idx, flag)?);
            }
            other => {
                return Err(CliError(format!(
                    "unknown option `{other}` for stage commit"
                )));
            }
        }
    }

    let version = version.ok_or_else(|| {
        CliError("missing required `--version` argument for stage commit".to_owned())
    })?;

    Ok(CliCommand::StageCommit {
        version,
        database_url,
        stage_dir,
        lock_path,
        checkpoint_path,
        schema,
    })
}

/// Parses `stage cleanup`: drop the scratch databases earlier verifications abandoned.
fn parse_stage_cleanup(tokens: &[&str]) -> Result<CliCommand, CliError> {
    let mut scratch_url = None;
    let mut database_url = None;
    let mut older_than_secs = DEFAULT_CLEANUP_AGE;
    let mut force = false;

    let mut idx = 0;
    while idx < tokens.len() {
        let flag = tokens[idx];
        if let Some(command) = universal_flag(flag) {
            return Ok(command);
        }
        match flag {
            "--scratch-url" => {
                scratch_url = Some(take_value(tokens, &mut idx, flag)?.to_owned());
            }
            "--database-url" => {
                database_url = Some(take_value(tokens, &mut idx, flag)?.to_owned());
            }
            "--older-than" => {
                older_than_secs = parse_seconds(flag, take_value(tokens, &mut idx, flag)?)?;
            }
            "--yes" | "-y" | "--force" => {
                force = true;
                idx += 1;
            }
            other => {
                return Err(CliError(format!(
                    "unknown option `{other}` for stage cleanup"
                )));
            }
        }
    }

    Ok(CliCommand::StageCleanup {
        scratch_url,
        database_url,
        older_than_secs,
        force,
    })
}

/// Parses a count of seconds supplied through `flag`.
fn parse_seconds(flag: &str, value: &str) -> Result<u64, CliError> {
    value
        .parse::<u64>()
        .map_err(|error| CliError(format!("invalid `{flag}` value `{value}`: {error}")))
}
