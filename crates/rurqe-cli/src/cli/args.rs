//! Command-line argument parsing.

use std::path::{Path, PathBuf};

use rurqe_migrate::checkpoint::CHECKPOINT_FILE;
use rurqe_migrate::stage::DeclarativeStage;
use rurqe_migrate::version::MigrationVersion;

/// The default directory holding the migration scripts.
const DEFAULT_MIGRATIONS_DIR: &str = "migrations";

/// The default directory holding the DDL scripts of the declarative stage.
const DEFAULT_STAGE_DIR: &str = DeclarativeStage::DIRECTORY;

/// The default file receiving the generated schema code.
const DEFAULT_SCHEMA_OUTPUT: &str = "src/schema.rs";

/// The default snapshot file recording the canonical schema hash.
const DEFAULT_LOCK_PATH: &str = "schema.lock";

/// The default age above which a scratch database counts as abandoned.
const DEFAULT_CLEANUP_AGE: u64 = crate::stage::postgres::DEFAULT_SCRATCH_MAX_AGE.as_secs();

/// Parsed CLI commands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CliCommand {
    MigrateUp {
        migrations_dir: PathBuf,
        checkpoint_path: PathBuf,
        database_url: Option<String>,
    },
    MigrateStatus {
        migrations_dir: PathBuf,
        database_url: Option<String>,
    },
    MigrateRepair {
        migrations_dir: PathBuf,
        database_url: Option<String>,
    },
    Baseline {
        version: MigrationVersion,
        description: String,
        database_url: Option<String>,
    },
    Codegen {
        database_url: Option<String>,
        output: PathBuf,
        lock_path: PathBuf,
        domain_types: bool,
    },
    Verify {
        database_url: Option<String>,
        schema_path: Option<PathBuf>,
        lock_path: PathBuf,
    },
    StageInit {
        stage_dir: PathBuf,
        version: MigrationVersion,
        description: String,
        database_url: Option<String>,
    },
    StageVerify {
        stage_dir: PathBuf,
        base_stage_dir: Option<PathBuf>,
        migrations_dir: PathBuf,
        checkpoint_path: PathBuf,
        scratch_url: Option<String>,
        database_url: Option<String>,
    },
    StageCommit {
        version: MigrationVersion,
        database_url: Option<String>,
        stage_dir: PathBuf,
        lock_path: PathBuf,
        checkpoint_path: PathBuf,
    },
    StageCleanup {
        scratch_url: Option<String>,
        database_url: Option<String>,
        older_than_secs: u64,
        force: bool,
    },
    Prune {
        before: MigrationVersion,
        migrations_dir: PathBuf,
        checkpoint_path: PathBuf,
        dry_run: bool,
    },
    Help,
    Version,
}

/// Errors occurring during CLI argument parsing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CliError(pub String);

impl std::fmt::Display for CliError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for CliError {}

/// The binary the CLI was invoked through.
///
/// Cargo dispatches custom subcommands as `cargo-<name> <name> ...`, so the
/// executable name selects the command when no arguments follow it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Entrypoint {
    CargoDbMigrate,
    CargoDbCodegen,
    CargoDbVerify,
    CargoDbBaseline,
    Direct,
}

impl Entrypoint {
    /// Classifies the executable name extracted from `argv[0]`.
    fn from_program(program: &str) -> Self {
        match program {
            "cargo-db-migrate" => Entrypoint::CargoDbMigrate,
            "cargo-db-codegen" => Entrypoint::CargoDbCodegen,
            "cargo-db-verify" => Entrypoint::CargoDbVerify,
            "cargo-db-baseline" => Entrypoint::CargoDbBaseline,
            _ => Entrypoint::Direct,
        }
    }

    /// The subcommand token this entrypoint implies, if any.
    fn command_name(self) -> Option<&'static str> {
        match self {
            Entrypoint::CargoDbMigrate => Some("db-migrate"),
            Entrypoint::CargoDbCodegen => Some("db-codegen"),
            Entrypoint::CargoDbVerify => Some("db-verify"),
            Entrypoint::CargoDbBaseline => Some("db-baseline"),
            Entrypoint::Direct => None,
        }
    }

    /// The command executed when the binary receives no arguments.
    fn default_command(self) -> Option<CliCommand> {
        match self {
            Entrypoint::CargoDbMigrate => Some(CliCommand::MigrateUp {
                migrations_dir: default_migrations_dir(),
                checkpoint_path: default_checkpoint_path(),
                database_url: None,
            }),
            Entrypoint::CargoDbCodegen => Some(CliCommand::Codegen {
                database_url: None,
                output: default_schema_output(),
                lock_path: default_lock_path(),
                domain_types: false,
            }),
            Entrypoint::CargoDbVerify => Some(CliCommand::Verify {
                database_url: None,
                schema_path: None,
                lock_path: default_lock_path(),
            }),
            Entrypoint::CargoDbBaseline | Entrypoint::Direct => None,
        }
    }
}

/// Default migration directory used when `--migrations-dir` is omitted.
fn default_migrations_dir() -> PathBuf {
    PathBuf::from(DEFAULT_MIGRATIONS_DIR)
}

/// Default declarative stage directory used when `--stage-dir` is omitted.
fn default_stage_dir() -> PathBuf {
    PathBuf::from(DEFAULT_STAGE_DIR)
}

/// Default checkpoint marker path used when `--checkpoint-path` is omitted.
fn default_checkpoint_path() -> PathBuf {
    PathBuf::from(CHECKPOINT_FILE)
}

/// Default generated schema path used when `--output` is omitted.
fn default_schema_output() -> PathBuf {
    PathBuf::from(DEFAULT_SCHEMA_OUTPUT)
}

/// Default schema snapshot path used when `--lock-path` is omitted.
fn default_lock_path() -> PathBuf {
    PathBuf::from(DEFAULT_LOCK_PATH)
}

/// Resolves the scratch database URL the golden equation is verified against.
///
/// `--scratch-url` names the scratch database explicitly and falls back to
/// `--database-url`. The `DATABASE_URL` environment variable is deliberately never
/// consulted: a PostgreSQL verification runs `CREATE DATABASE` and `DROP DATABASE`
/// as the connected role, so an implicit target routinely names the production
/// server the variable points at in CI and on a developer workstation. No implicit
/// sandbox is chosen either: the verification replays schema DDL into that target,
/// so silently selecting an in-memory SQLite engine would exercise a different
/// engine than the project targets and hide the substitution from CI.
pub(crate) fn resolve_equation_url(supplied: Option<String>) -> Result<String, CliError> {
    supplied
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            CliError(
                "no scratch database URL provided: pass `--scratch-url <url>` (or \
                 `--database-url <url>`); the golden equation needs a disposable target such as \
                 `:memory:`, an empty SQLite file, or a PostgreSQL URL, and never reads \
                 DATABASE_URL"
                    .to_owned(),
            )
        })
}

/// Resolves the server `stage cleanup` sweeps the abandoned scratch databases on.
pub(crate) fn resolve_cleanup_url(supplied: Option<String>) -> Result<String, CliError> {
    scratch_url(supplied).ok_or_else(|| {
        CliError(
            "no scratch database URL provided: pass `--scratch-url <url>` (or \
             `--database-url <url>`, or set the DATABASE_URL environment variable); the sweep \
             names the PostgreSQL server that keeps the scratch databases"
                .to_owned(),
        )
    })
}

/// Applies the shared precedence of a scratch database URL.
///
/// The flag supplied by the caller wins over the `DATABASE_URL` environment
/// variable, and a blank value names nothing at all.
fn scratch_url(supplied: Option<String>) -> Option<String> {
    supplied
        .or_else(|| std::env::var("DATABASE_URL").ok())
        .filter(|value| !value.trim().is_empty())
}

/// Borrows the value following the flag at `*idx`, advancing past both.
///
/// A missing value is reported against the flag itself instead of degrading
/// into the misleading `unknown option` diagnostic.
fn take_value<'a>(tokens: &[&'a str], idx: &mut usize, flag: &str) -> Result<&'a str, CliError> {
    let value = tokens
        .get(*idx + 1)
        .copied()
        .ok_or_else(|| CliError(format!("option `{flag}` requires a value")))?;
    *idx += 2;
    Ok(value)
}

/// Parses command-line arguments into a typed [`CliCommand`].
pub fn parse_args(args: &[String]) -> Result<CliCommand, CliError> {
    let tokens: Vec<&str> = args.iter().map(String::as_str).collect();

    let Some(program) = tokens.first().copied() else {
        return Ok(CliCommand::Help);
    };

    let program = Path::new(program)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("");

    let entrypoint = Entrypoint::from_program(program);

    // Skip the optional `rurqe`/`db` token passed by `cargo rurqe ...`.
    let rest = &tokens[1..];
    let rest = match rest.first() {
        Some(token) if *token == "rurqe" || *token == "db" => &rest[1..],
        _ => rest,
    };

    // Cargo dispatches custom subcommands as `cargo-<name> <name> ...`, so
    // drop the duplicated subcommand token before parsing the arguments.
    let rest = match entrypoint.command_name() {
        Some(name) if rest.first().is_some_and(|token| *token == name) => &rest[1..],
        _ => rest,
    };

    // A cargo entrypoint forces its subcommand from the executable name, so the
    // universal flags are handled here: `cargo db-migrate --help` would otherwise
    // reach the migrate flag loop and be rejected as an unknown option. `-V`
    // names the program version unambiguously, while `--version` stays available
    // to `baseline`, which demands it as its own argument.
    if let Some(first) = rest.first().copied() {
        match first {
            "--help" | "-h" => return Ok(CliCommand::Help),
            "-V" => return Ok(CliCommand::Version),
            "--version" if entrypoint == Entrypoint::CargoDbBaseline => {}
            "--version" | "-v" => return Ok(CliCommand::Version),
            _ => {}
        }
    }

    let Some((first, remaining)) = rest.split_first() else {
        return Ok(entrypoint.default_command().unwrap_or(CliCommand::Help));
    };

    // Direct invocations carry the subcommand as the first argument, while
    // cargo-dispatched binaries force it from the entrypoint name.
    let (subcmd, tokens): (&str, &[&str]) = match entrypoint.command_name() {
        Some(name) => (name, rest),
        None => (*first, remaining),
    };

    match subcmd {
        "--help" | "-h" | "help" => Ok(CliCommand::Help),
        "--version" | "-v" | "version" => Ok(CliCommand::Version),

        "db-migrate" | "migrate" => {
            let (action, tokens): (&str, &[&str]) = match tokens.split_first() {
                Some((action, tail)) if !action.starts_with('-') => (*action, tail),
                _ => ("up", tokens),
            };

            let mut migrations_dir = default_migrations_dir();
            let mut checkpoint_path = default_checkpoint_path();
            let mut database_url = None;

            let mut idx = 0;
            while idx < tokens.len() {
                let flag = tokens[idx];
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
                    other => {
                        return Err(CliError(format!("unknown option `{other}` for migrate")));
                    }
                }
            }

            match action {
                "up" => Ok(CliCommand::MigrateUp {
                    migrations_dir,
                    checkpoint_path,
                    database_url,
                }),
                "status" => Ok(CliCommand::MigrateStatus {
                    migrations_dir,
                    database_url,
                }),
                "repair" => Ok(CliCommand::MigrateRepair {
                    migrations_dir,
                    database_url,
                }),
                other => Err(CliError(format!(
                    "unknown migrate action `{other}` (expected `up`, `status`, or `repair`)"
                ))),
            }
        }

        "db-baseline" | "baseline" => {
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

        "db-codegen" | "codegen" => {
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

        "db-verify" | "verify" => {
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

        "db-stage" | "stage" => parse_stage(tokens),

        "db-prune" | "prune" => parse_prune(tokens),

        other => Err(CliError(format!("unknown command `{other}`"))),
    }
}

/// Parses a `stage <action>` invocation into its typed command.
fn parse_stage(tokens: &[&str]) -> Result<CliCommand, CliError> {
    let action = match tokens.first() {
        Some(action) if !action.starts_with('-') => *action,
        _ => {
            return Err(CliError(
                "missing stage action (expected `init`, `verify`, `commit`, or `cleanup`)"
                    .to_owned(),
            ))
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

    let mut idx = 0;
    while idx < tokens.len() {
        let flag = tokens[idx];
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

    let mut idx = 0;
    while idx < tokens.len() {
        let flag = tokens[idx];
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

/// Parses `prune`: drop migration scripts the checkpoint already folded in.
fn parse_prune(tokens: &[&str]) -> Result<CliCommand, CliError> {
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

/// Parses a migration version supplied through `flag`.
fn parse_version(flag: &str, value: &str) -> Result<MigrationVersion, CliError> {
    value
        .parse::<MigrationVersion>()
        .map_err(|error| CliError(format!("invalid `{flag}` value `{value}`: {error}")))
}
