//! Command-line argument parsing.

use std::path::{Path, PathBuf};

use rurqe_migrate::checkpoint::CHECKPOINT_FILE;
use rurqe_migrate::stage::DeclarativeStage;
use rurqe_migrate::version::MigrationVersion;

mod baseline;
mod codegen;
mod migrate;
mod prune;
mod stage;

/// The default directory holding the migration scripts.
const DEFAULT_MIGRATIONS_DIR: &str = "migrations";

/// The default directory holding the DDL scripts of the declarative stage.
const DEFAULT_STAGE_DIR: &str = DeclarativeStage::DIRECTORY;

/// The default file receiving the generated schema code.
const DEFAULT_SCHEMA_OUTPUT: &str = "src/schema.rs";

/// The default snapshot file recording the canonical schema hash.
const DEFAULT_LOCK_PATH: &str = "schema.lock";

/// Parsed CLI commands.
///
/// The commands that compare catalogs carry the schema selected with `--schema`,
/// which the dispatcher resolves against the `schema` key of `rurqe.toml` before
/// any catalog is read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CliCommand {
    MigrateUp {
        migrations_dir: PathBuf,
        checkpoint_path: PathBuf,
        database_url: Option<String>,
        schema: Option<String>,
    },
    MigrateStatus {
        migrations_dir: PathBuf,
        database_url: Option<String>,
        schema: Option<String>,
    },
    MigrateRepair {
        migrations_dir: PathBuf,
        database_url: Option<String>,
        schema: Option<String>,
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
        schema: Option<String>,
    },
    Verify {
        database_url: Option<String>,
        schema_path: Option<PathBuf>,
        lock_path: PathBuf,
        schema: Option<String>,
    },
    StageInit {
        stage_dir: PathBuf,
        version: MigrationVersion,
        description: String,
        database_url: Option<String>,
        schema: Option<String>,
    },
    StageVerify {
        stage_dir: PathBuf,
        base_stage_dir: Option<PathBuf>,
        migrations_dir: PathBuf,
        checkpoint_path: PathBuf,
        scratch_url: Option<String>,
        database_url: Option<String>,
        schema: Option<String>,
    },
    StageCommit {
        version: MigrationVersion,
        database_url: Option<String>,
        stage_dir: PathBuf,
        lock_path: PathBuf,
        checkpoint_path: PathBuf,
        schema: Option<String>,
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

/// Why a per-action parsing walk stopped before producing a command.
///
/// The universal flags are answered in every position they can occupy, including
/// the position of a value, so a walk reports that answer as an interruption
/// instead of consuming the flag as data: whoever started the walk collapses the
/// interruption into the result the caller sees.
#[derive(Debug, Clone, PartialEq, Eq)]
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
                schema: None,
            }),
            Entrypoint::CargoDbCodegen => Some(CliCommand::Codegen {
                database_url: None,
                output: default_schema_output(),
                lock_path: default_lock_path(),
                domain_types: false,
                schema: None,
            }),
            Entrypoint::CargoDbVerify => Some(CliCommand::Verify {
                database_url: None,
                schema_path: None,
                lock_path: default_lock_path(),
                schema: None,
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
/// into the misleading `unknown option` diagnostic. A universal flag standing
/// where the value belongs is answered exactly like one leading the argument
/// list — it never becomes the database URL, the file name, or the path the
/// command would otherwise use — while a token beginning with `--` is refused
/// against the flag that stayed without its value.
fn take_value<'a>(tokens: &[&'a str], idx: &mut usize, flag: &str) -> Result<&'a str, Interrupt> {
    let value = tokens
        .get(*idx + 1)
        .copied()
        .ok_or_else(|| CliError(format!("option `{flag}` requires a value")))?;

    // `--version` is deliberately not answered here: `baseline`, `stage init`,
    // and `stage commit` take it as their own argument, so a bare `--version`
    // where a value belongs stays ambiguous and is refused like any other
    // option-shaped token.
    if let Some(command) = universal_flag_beside_version(value) {
        return Err(Interrupt::universal(command));
    }

    if value.starts_with("--") {
        return Err(CliError(format!(
            "option `{flag}` requires a value, but `{value}` is another option"
        ))
        .into());
    }

    *idx += 2;
    Ok(value)
}

/// Borrows the schema name following `--schema`, rejecting a blank one.
///
/// The name decides which catalog every digest of a command describes, so a
/// blank value is refused instead of silently selecting nothing.
fn take_schema(tokens: &[&str], idx: &mut usize, flag: &str) -> Result<String, Interrupt> {
    let value = take_value(tokens, idx, flag)?;
    let name = value.trim();

    if name.is_empty() {
        return Err(CliError(format!(
            "option `{flag}` requires a database schema name"
        ))
        .into());
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

    let parsed: Result<CliCommand, Interrupt> = match subcmd {
        "--help" | "-h" | "help" => Ok(CliCommand::Help),
        "--version" | "-v" | "version" => Ok(CliCommand::Version),

        "db-migrate" | "migrate" => migrate::parse_migrate(tokens),
        "db-baseline" | "baseline" => baseline::parse_baseline(tokens),
        "db-codegen" | "codegen" => codegen::parse_codegen(tokens),
        "db-verify" | "verify" => codegen::parse_verify(tokens),
        "db-stage" | "stage" => stage::parse_stage(tokens),
        "db-prune" | "prune" => prune::parse_prune(tokens),

        other => Err(CliError(format!("unknown command `{other}`")).into()),
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
        .map_err(|error| CliError(format!("invalid `{flag}` value `{value}`: {error}")))
}
