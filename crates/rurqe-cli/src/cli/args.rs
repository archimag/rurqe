//! Command-line argument parsing.

use std::path::{Path, PathBuf};

use rurqe_migrate::version::MigrationVersion;

use super::error::CliError;

mod baseline;
mod codegen;
mod migrate;
mod prune;
mod stage;

/// Parsed CLI commands.
///
/// The commands that compare catalogs carry the schema selected with `--schema`,
/// which the dispatcher resolves against the `primary_schema` key of `rurqe.toml`
/// before any catalog is read.
///
/// Every command carries the configuration file named by `--config` and the
/// artifact paths named by the flags of its own action. A path field holds a value
/// only when its flag was supplied, so the dispatcher resolves each of them through
/// the one cascade it applies everywhere: the explicit flag, then the key of
/// `rurqe.toml`, then the conventional path beneath the artifact directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CliCommand {
    MigrateUp {
        config: Option<PathBuf>,
        database_url: Option<String>,
        schema: Option<String>,
    },
    MigrateStatus {
        config: Option<PathBuf>,
        database_url: Option<String>,
        schema: Option<String>,
    },
    MigrateRepair {
        config: Option<PathBuf>,
        database_url: Option<String>,
        schema: Option<String>,
    },
    MigrateClean {
        config: Option<PathBuf>,
        database_url: Option<String>,
        schema: Option<String>,
        force: bool,
    },
    Baseline {
        config: Option<PathBuf>,
        version: MigrationVersion,
        description: String,
        database_url: Option<String>,
    },
    Codegen {
        config: Option<PathBuf>,
        database_url: Option<String>,
        output: Option<PathBuf>,
        lock_path: Option<PathBuf>,
        domain_types: bool,
        schema: Option<String>,
    },
    Verify {
        config: Option<PathBuf>,
        database_url: Option<String>,
        schema_path: Option<PathBuf>,
        lock_path: Option<PathBuf>,
        schema: Option<String>,
    },
    StageInit {
        config: Option<PathBuf>,
        stage_dir: Option<PathBuf>,
        version: MigrationVersion,
        description: String,
        database_url: Option<String>,
        schema: Option<String>,
    },
    StageVerify {
        config: Option<PathBuf>,
        stage_dir: Option<PathBuf>,
        base_stage_dir: Option<PathBuf>,
        migrations_dir: Option<PathBuf>,
        checkpoint_path: Option<PathBuf>,
        scratch_url: Option<String>,
        database_url: Option<String>,
        schema: Option<String>,
    },
    StageCommit {
        config: Option<PathBuf>,
        version: MigrationVersion,
        database_url: Option<String>,
        scratch_url: Option<String>,
        stage_dir: Option<PathBuf>,
        lock_path: Option<PathBuf>,
        checkpoint_path: Option<PathBuf>,
        schema: Option<String>,
    },
    StageCleanup {
        config: Option<PathBuf>,
        scratch_url: Option<String>,
        database_url: Option<String>,
        older_than_secs: u64,
        force: bool,
    },
    Prune {
        config: Option<PathBuf>,
        before: MigrationVersion,
        migrations_dir: Option<PathBuf>,
        checkpoint_path: Option<PathBuf>,
        dry_run: bool,
    },
    Help,
    Version,
}

impl CliCommand {
    /// The configuration file the invocation named with `--config`.
    ///
    /// `None` means the invocation named none, so the dispatcher falls back to the
    /// conventional `rurqe.toml` of the project it runs in. The accessor reads the
    /// proposed file without destructuring every action of the command tree, which
    /// is what lets the dispatcher and the regressions share one answer.
    pub fn config_path(&self) -> Option<&Path> {
        match self {
            CliCommand::MigrateUp { config, .. }
            | CliCommand::MigrateStatus { config, .. }
            | CliCommand::MigrateRepair { config, .. }
            | CliCommand::MigrateClean { config, .. }
            | CliCommand::Baseline { config, .. }
            | CliCommand::Codegen { config, .. }
            | CliCommand::Verify { config, .. }
            | CliCommand::StageInit { config, .. }
            | CliCommand::StageVerify { config, .. }
            | CliCommand::StageCommit { config, .. }
            | CliCommand::StageCleanup { config, .. }
            | CliCommand::Prune { config, .. } => config.as_deref(),
            CliCommand::Help | CliCommand::Version => None,
        }
    }
}

/// Why a per-action parsing walk stopped before producing a command.
///
/// The universal flags are answered in every position they can occupy, including
/// the position of a value, so a walk reports that answer as an interruption
/// instead of consuming the flag as data: whoever started the walk collapses the
/// interruption into the result the caller sees.
#[derive(Debug)]
pub(crate) enum Interrupt {
    /// The walk reached a universal flag and answers it instead of its action.
    ///
    /// The answer is boxed so every fallible step of a parse keeps an `Err`
    /// payload as small as the diagnostic it competes with: a command built out
    /// of paths, URLs, and a schema name is several times larger than the
    /// `CliError` the same steps report.
    Universal(Box<CliCommand>),
    /// The arguments do not describe a command.
    Malformed(CliError),
}

impl From<CliError> for Interrupt {
    fn from(error: CliError) -> Self {
        Interrupt::Malformed(error)
    }
}

impl Interrupt {
    /// Interrupts a walk that reached a universal flag and answers it.
    fn universal(command: CliCommand) -> Self {
        Interrupt::Universal(Box::new(command))
    }

    /// Collapses the interruption into the result `parse_args` reports.
    fn resolve(self) -> Result<CliCommand, CliError> {
        match self {
            Interrupt::Universal(command) => Ok(*command),
            Interrupt::Malformed(error) => Err(error),
        }
    }
}

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
    /// Classifies the file stem of the executable extracted from `argv[0]`.
    ///
    /// The stem carries no extension, so the `.exe` suffix a Windows binary
    /// arrives with never hides the subcommand the invocation names.
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
    ///
    /// Every path of the forced command stays unset, so the dispatcher resolves it
    /// through the same cascade an explicit invocation uses.
    fn default_command(self) -> Option<CliCommand> {
        match self {
            Entrypoint::CargoDbMigrate => Some(CliCommand::MigrateUp {
                config: None,
                database_url: None,
                schema: None,
            }),
            Entrypoint::CargoDbCodegen => Some(CliCommand::Codegen {
                config: None,
                database_url: None,
                output: None,
                lock_path: None,
                domain_types: false,
                schema: None,
            }),
            Entrypoint::CargoDbVerify => Some(CliCommand::Verify {
                config: None,
                database_url: None,
                schema_path: None,
                lock_path: None,
                schema: None,
            }),
            Entrypoint::CargoDbBaseline | Entrypoint::Direct => None,
        }
    }
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
            CliError::message(
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
    scratch_url(supplied)?.ok_or_else(|| {
        CliError::message(
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
/// variable, and a blank value names nothing at all. The environment is read only
/// when the flag named no target, through the reader of the connection module, so
/// a variable that is present but not valid UTF-8 is reported instead of being
/// folded into the "nothing supplied" answer [`resolve_cleanup_url`] renders.
fn scratch_url(supplied: Option<String>) -> Result<Option<String>, CliError> {
    let resolved = match supplied {
        Some(url) => Some(url),
        None => super::connection::environment_database_url()?,
    };

    Ok(resolved.filter(|value| !value.trim().is_empty()))
}

/// Borrows the value following the flag at `*idx`, advancing past both.
///
/// A missing value is reported against the flag itself instead of degrading
/// into the misleading `unknown option` diagnostic. A universal flag standing
/// where the value belongs is answered exactly like one leading the argument
/// list — it never becomes the database URL, the file name, or the path the
/// command would otherwise use — while a token beginning with `--` is refused
/// against the flag that stayed without its value.
fn take_value<'a>(tokens: &[&'a str], idx: &mut usize, flag: &str) -> Result<&'a str, Interrupt> {
    let value = tokens
        .get(*idx + 1)
        .copied()
        .ok_or_else(|| CliError::message(format!("option `{flag}` requires a value")))?;

    // `--version` is deliberately not answered here: `baseline`, `stage init`,
    // and `stage commit` take it as their own argument, so a bare `--version`
    // where a value belongs stays ambiguous and is refused like any other
    // option-shaped token.
    if let Some(command) = universal_flag_beside_version(value) {
        return Err(Interrupt::universal(command));
    }

    if value.starts_with("--") {
        return Err(CliError::message(format!(
            "option `{flag}` requires a value, but `{value}` is another option"
        ))
        .into());
    }

    *idx += 2;
    Ok(value)
}

/// Borrows the path following the flag at `*idx`, advancing past both.
///
/// A path is taken exactly like any other value, so a universal flag standing where
/// the path belongs is answered instead of becoming a file name, and an
/// option-shaped token is refused against the flag that stayed without its value.
fn take_path(tokens: &[&str], idx: &mut usize, flag: &str) -> Result<PathBuf, Interrupt> {
    Ok(PathBuf::from(take_value(tokens, idx, flag)?))
}

/// Borrows the schema name following `--schema`, rejecting a blank one.
///
/// The name decides which catalog every digest of a command describes, so a
/// blank value is refused instead of silently selecting nothing.
fn take_schema(tokens: &[&str], idx: &mut usize, flag: &str) -> Result<String, Interrupt> {
    let value = take_value(tokens, idx, flag)?;
    let name = value.trim();

    if name.is_empty() {
        return Err(CliError::message(format!("option `{flag}` requires a database schema name")).into());
    }

    Ok(name.to_owned())
}

/// Recognizes the universal flags every subcommand answers alike.
///
/// The per-action parsers consult this while walking their own tokens, so a
/// trailing `--help` or `-V` is answered exactly like a leading one instead of
/// being reported as an unknown option of the action it follows.
fn universal_flag(token: &str) -> Option<CliCommand> {
    match token {
        "--help" | "-h" => Some(CliCommand::Help),
        "-V" | "--version" | "-v" => Some(CliCommand::Version),
        _ => None,
    }
}

/// Recognizes the universal flags beside `--version`.
///
/// `--version` names an argument of `baseline`, `stage init`, and `stage
/// commit`, so those parsers keep it for themselves and answer only the flags
/// that carry no meaning of their own.
fn universal_flag_beside_version(token: &str) -> Option<CliCommand> {
    match token {
        "--help" | "-h" => Some(CliCommand::Help),
        "-V" | "-v" => Some(CliCommand::Version),
        _ => None,
    }
}

/// Parses command-line arguments into a typed [`CliCommand`].
///
/// `args` has the shape of a process argument vector: its first element names
/// the program the invocation was started through and is classified rather than
/// parsed — its file stem selects the subcommand of a `cargo-<name>` dispatch —
/// while every element after it is the invocation itself. A leading `rurqe` or
/// `db` token, and the subcommand cargo repeats to a `cargo-<name>` binary, are
/// dropped before the action is parsed.
///
/// The program name is therefore load-bearing. A vector that opens with the
/// subcommand instead of it, such as `["prune", "--before", "2"]`, loses
/// `prune` to the program position and reports `--before` as an unknown
/// command. A caller holding subcommand-style arguments supplies a program name
/// of its own, or passes a process argument vector such as [`std::env::args`]
/// through unchanged.
pub fn parse_args(args: &[String]) -> Result<CliCommand, CliError> {
    let tokens: Vec<&str> = args.iter().map(String::as_str).collect();

    let Some(program) = tokens.first().copied() else {
        return Ok(CliCommand::Help);
    };

    // The executable is classified by its file stem, because a produced binary
    // carries a platform suffix: on Windows `argv[0]` of a cargo-dispatched
    // subcommand ends in `.exe`, and an exact match against the bare name would
    // degrade every such invocation into a direct one. The stem is lexical and
    // identical everywhere, so one argument list classifies alike on every
    // platform.
    let program = Path::new(program)
        .file_stem()
        .and_then(|name| name.to_str())
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
        // The universal flags are answered through the same tables the
        // per-action flag loops and `take_value` consult, so the leading
        // position and every other one agree by construction.
        let answered = if entrypoint == Entrypoint::CargoDbBaseline {
            universal_flag_beside_version(first)
        } else {
            universal_flag(first)
        };
        if let Some(command) = answered {
            return Ok(command);
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

    let parsed: Result<CliCommand, Interrupt> = match subcmd {
        "--help" | "-h" | "help" => Ok(CliCommand::Help),
        "--version" | "-v" | "version" => Ok(CliCommand::Version),

        "db-migrate" | "migrate" => migrate::parse_migrate(tokens),
        "db-baseline" | "baseline" => baseline::parse_baseline(tokens),
        "db-codegen" | "codegen" => codegen::parse_codegen(tokens),
        "db-verify" | "verify" => codegen::parse_verify(tokens),
        "db-stage" | "stage" => stage::parse_stage(tokens),
        "db-prune" | "prune" => prune::parse_prune(tokens),

        other => Err(CliError::message(format!("unknown command `{other}`")).into()),
    };

    // A walk that answered a universal flag reports the answer it reached, while
    // an unusable argument list reports the diagnostic it stopped on.
    match parsed {
        Ok(command) => Ok(command),
        Err(interrupt) => interrupt.resolve(),
    }
}

/// Parses a migration version supplied through `flag`.
fn parse_version(flag: &str, value: &str) -> Result<MigrationVersion, CliError> {
    value
        .parse::<MigrationVersion>()
        .map_err(|error| CliError::message(format!("invalid `{flag}` value `{value}`: {error}")))
}
