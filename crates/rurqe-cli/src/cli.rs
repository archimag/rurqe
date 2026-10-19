//! Command-line argument parser and execution coordinator.
//!
//! Supports standalone binary execution (`rurqe ...`) and Cargo subcommand
//! dispatching (`cargo db-migrate ...`, `cargo db-codegen ...`, `cargo db-verify ...`).

use std::path::{Path, PathBuf};

use rurqe_migrate::runner::{MigrationBackend, MigrationStatusKind, Migrator};
use rurqe_migrate::version::MigrationVersion;

use crate::codegen::{generate_schema_rs, CodegenOptions};
use crate::drift::verify_drift;
use crate::driver::{PostgresMigrationBackend, SqliteMigrationBackend};
use crate::introspect::{detect_dialect, introspect_database, DatabaseDialect};
use crate::loader::load_migrations_from_dir;

/// Parsed CLI commands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CliCommand {
    MigrateUp {
        migrations_dir: PathBuf,
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
    },
    Verify {
        database_url: Option<String>,
        schema_path: Option<PathBuf>,
        lock_path: PathBuf,
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

/// Parses command-line arguments into a typed [`CliCommand`].
pub fn parse_args(args: &[String]) -> Result<CliCommand, CliError> {
    let mut tokens: Vec<&str> = args.iter().map(String::as_str).collect();

    if tokens.is_empty() {
        return Ok(CliCommand::Help);
    }

    // Determine command name from argv[0] or first argument
    let program = Path::new(tokens[0])
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("");

    tokens.remove(0);

    // If invoked through cargo: e.g. `cargo rurqe ...`
    if !tokens.is_empty() && (tokens[0] == "rurqe" || tokens[0] == "db") {
        tokens.remove(0);
    }

    // Cargo invokes custom subcommand binaries as `cargo-<name> <name> ...`,
    // so strip the duplicated subcommand token passed as argv[1].
    if let Some(sub) = program.strip_prefix("cargo-") {
        if tokens.first() == Some(&sub) {
            tokens.remove(0);
        }
    }

    if tokens.is_empty() {
        if program == "cargo-db-migrate" {
            return Ok(CliCommand::MigrateUp {
                migrations_dir: PathBuf::from("migrations"),
                database_url: None,
            });
        }
        if program == "cargo-db-codegen" {
            return Ok(CliCommand::Codegen {
                database_url: None,
                output: PathBuf::from("src/schema.rs"),
                lock_path: PathBuf::from("schema.lock"),
            });
        }
        if program == "cargo-db-verify" {
            return Ok(CliCommand::Verify {
                database_url: None,
                schema_path: None,
                lock_path: PathBuf::from("schema.lock"),
            });
        }
        return Ok(CliCommand::Help);
    }

    let subcmd = if program == "cargo-db-migrate" {
        "db-migrate"
    } else if program == "cargo-db-codegen" {
        "db-codegen"
    } else if program == "cargo-db-verify" {
        "db-verify"
    } else if program == "cargo-db-baseline" {
        "db-baseline"
    } else {
        tokens.remove(0)
    };

    match subcmd {
        "--help" | "-h" | "help" => Ok(CliCommand::Help),
        "--version" | "-v" | "version" => Ok(CliCommand::Version),

        "db-migrate" | "migrate" => {
            let action = if !tokens.is_empty() && !tokens[0].starts_with('-') {
                tokens.remove(0)
            } else {
                "up"
            };

            let mut migrations_dir = PathBuf::from("migrations");
            let mut database_url = None;

            let mut idx = 0;
            while idx < tokens.len() {
                match tokens[idx] {
                    "--migrations-dir" if idx + 1 < tokens.len() => {
                        migrations_dir = PathBuf::from(tokens[idx + 1]);
                        idx += 2;
                    }
                    "--database-url" if idx + 1 < tokens.len() => {
                        database_url = Some(tokens[idx + 1].to_owned());
                        idx += 2;
                    }
                    other => {
                        return Err(CliError(format!("unknown option `{other}` for migrate")));
                    }
                }
            }

            match action {
                "up" => Ok(CliCommand::MigrateUp {
                    migrations_dir,
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
                match tokens[idx] {
                    "--version" if idx + 1 < tokens.len() => {
                        version_str = Some(tokens[idx + 1]);
                        idx += 2;
                    }
                    "--description" if idx + 1 < tokens.len() => {
                        description = tokens[idx + 1].to_owned();
                        idx += 2;
                    }
                    "--database-url" if idx + 1 < tokens.len() => {
                        database_url = Some(tokens[idx + 1].to_owned());
                        idx += 2;
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
            let mut output = PathBuf::from("src/schema.rs");
            let mut lock_path = PathBuf::from("schema.lock");

            let mut idx = 0;
            while idx < tokens.len() {
                match tokens[idx] {
                    "--database-url" if idx + 1 < tokens.len() => {
                        database_url = Some(tokens[idx + 1].to_owned());
                        idx += 2;
                    }
                    "--output" | "-o" if idx + 1 < tokens.len() => {
                        output = PathBuf::from(tokens[idx + 1]);
                        idx += 2;
                    }
                    "--lock-path" if idx + 1 < tokens.len() => {
                        lock_path = PathBuf::from(tokens[idx + 1]);
                        idx += 2;
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
            })
        }

        "db-verify" | "verify" => {
            let mut database_url = None;
            let mut schema_path = None;
            let mut lock_path = PathBuf::from("schema.lock");

            let mut idx = 0;
            while idx < tokens.len() {
                match tokens[idx] {
                    "--database-url" if idx + 1 < tokens.len() => {
                        database_url = Some(tokens[idx + 1].to_owned());
                        idx += 2;
                    }
                    "--schema-path" if idx + 1 < tokens.len() => {
                        schema_path = Some(PathBuf::from(tokens[idx + 1]));
                        idx += 2;
                    }
                    "--lock-path" if idx + 1 < tokens.len() => {
                        lock_path = PathBuf::from(tokens[idx + 1]);
                        idx += 2;
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

        other => Err(CliError(format!("unknown command `{other}`"))),
    }
}

/// Opens the migration backend for `database_url` and evaluates `$run` against it.
///
/// SQLite and PostgreSQL backends are distinct static types, so the backend is
/// constructed once per command through the shared [`connect`] helper instead of
/// repeating the dialect match and connection logic by hand.
macro_rules! with_migration_backend {
    ($database_url:expr, |$backend:ident| $run:expr) => {{
        let url = resolve_database_url($database_url)?;
        match connect(&url)? {
            Connection::Sqlite(conn) => {
                let mut $backend = SqliteMigrationBackend::new(conn);
                $run
            }
            Connection::Postgres(client) => {
                let mut $backend = PostgresMigrationBackend::new(client);
                $run
            }
        }
    }};
}

/// Runs the CLI toolchain and prints execution outputs.
pub fn run_cli(args: Vec<String>) -> Result<(), CliError> {
    let cmd = parse_args(&args)?;

    match cmd {
        CliCommand::Help => {
            print_help();
            Ok(())
        }
        CliCommand::Version => {
            println!("rurqe-cli {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        CliCommand::MigrateUp {
            migrations_dir,
            database_url,
        } => {
            with_migration_backend!(database_url, |backend| {
                apply_migrations(&mut backend, &migrations_dir)
            })
        }
        CliCommand::MigrateStatus {
            migrations_dir,
            database_url,
        } => {
            with_migration_backend!(database_url, |backend| {
                report_status(&mut backend, &migrations_dir)
            })
        }
        CliCommand::MigrateRepair {
            migrations_dir,
            database_url,
        } => {
            with_migration_backend!(database_url, |backend| {
                repair_history(&mut backend, &migrations_dir)
            })
        }
        CliCommand::Baseline {
            version,
            description,
            database_url,
        } => {
            with_migration_backend!(database_url, |backend| {
                baseline_database(&mut backend, version, &description)
            })
        }
        CliCommand::Codegen {
            database_url,
            output,
            lock_path,
        } => {
            let url = resolve_database_url(database_url)?;
            let catalog = introspect_database(&url)?;
            let code = generate_schema_rs(&catalog, &CodegenOptions::default())
                .map_err(|err| CliError(format!("failed to generate schema code: {err}")))?;
            write_file(&output, &code)?;
            write_file(&lock_path, &catalog.to_lock())?;
            println!(
                "Generated schema to {} and snapshot to {} (hash {})",
                output.display(),
                lock_path.display(),
                catalog.schema_hash()
            );
            Ok(())
        }
        CliCommand::Verify {
            database_url,
            schema_path,
            lock_path,
        } => {
            let url = resolve_database_url(database_url)?;
            let expected_lock = std::fs::read_to_string(&lock_path).map_err(|e| {
                CliError(format!(
                    "failed to read schema lock `{}`: {e}",
                    lock_path.display()
                ))
            })?;
            let schema_code = resolve_schema_code(schema_path)?;
            let catalog = introspect_database(&url)?;
            verify_drift(&catalog, &expected_lock, schema_code.as_deref())
                .map_err(|err| CliError(err.to_string()))?;
            println!("Schema verification passed against {}", lock_path.display());
            Ok(())
        }
    }
}

fn cli_error(err: impl std::fmt::Display) -> CliError {
    CliError(err.to_string())
}

/// Resolves the target database URL from the flag or the `DATABASE_URL` environment variable.
fn resolve_database_url(supplied: Option<String>) -> Result<String, CliError> {
    supplied
        .or_else(|| std::env::var("DATABASE_URL").ok())
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            CliError(
                "no database URL provided: pass `--database-url <url>` or set the \
                 DATABASE_URL environment variable"
                    .to_owned(),
            )
        })
}

/// Reads the generated schema code used for `SCHEMA_HASH` verification.
///
/// An explicitly supplied `--schema-path` must resolve to a readable file, so a
/// deleted or misconfigured schema file can never silently pass CI. When the
/// flag is omitted, the conventional `src/schema.rs` is used if present and
/// verification otherwise falls back to the lock snapshot alone with a warning.
fn resolve_schema_code(explicit: Option<PathBuf>) -> Result<Option<String>, CliError> {
    let Some(path) = explicit else {
        let default = PathBuf::from("src/schema.rs");
        if !default.exists() {
            eprintln!(
                "warning: schema code `{}` not found; verifying against the lock snapshot only \
                 (pass `--schema-path` to require the generated code)",
                default.display()
            );
            return Ok(None);
        }
        return read_schema_code(&default).map(Some);
    };

    read_schema_code(&path).map(Some)
}

fn read_schema_code(path: &Path) -> Result<String, CliError> {
    std::fs::read_to_string(path).map_err(|e| {
        CliError(format!(
            "failed to read schema code `{}`: {e}",
            path.display()
        ))
    })
}

/// Replaces the userinfo section of a connection URL with `***`.
///
/// Connection strings routinely embed credentials (`postgres://user:secret@host/db`),
/// and CLI failures end up in CI logs. Only the authority section between `://` and
/// the first `/`, `?` or `#` is touched, so SQLite file paths, hosts and query
/// parameters are preserved verbatim.
pub fn redact_database_url(url: &str) -> String {
    let Some(scheme_end) = url.find("://") else {
        return url.to_owned();
    };

    let authority_start = scheme_end + 3;
    let authority = &url[authority_start..];
    let authority_len = authority
        .find(['/', '?', '#'])
        .unwrap_or(authority.len());

    let Some(at) = authority[..authority_len].rfind('@') else {
        return url.to_owned();
    };

    if at == 0 {
        return url.to_owned();
    }

    format!(
        "{}***@{}",
        &url[..authority_start],
        &url[authority_start + at + 1..]
    )
}

/// A live database connection opened from a URL or filesystem path.
pub(crate) enum Connection {
    /// SQLite database handle.
    Sqlite(rusqlite::Connection),
    /// PostgreSQL client.
    Postgres(postgres::Client),
}

/// Opens a connection to the database identified by `url`, detecting its engine.
///
/// This is the single entry point for scheme stripping, `:memory:` handling and
/// connection error wording, so the CLI commands and catalog introspection can
/// never drift apart.
pub(crate) fn connect(url: &str) -> Result<Connection, CliError> {
    match detect_dialect(Some(url)) {
        DatabaseDialect::Sqlite => Ok(Connection::Sqlite(open_sqlite(url)?)),
        DatabaseDialect::Postgres => Ok(Connection::Postgres(connect_postgres(url)?)),
    }
}

/// Opens a SQLite connection from a URL or plain filesystem path.
fn open_sqlite(url: &str) -> Result<rusqlite::Connection, CliError> {
    let path = url
        .strip_prefix("sqlite://")
        .or_else(|| url.strip_prefix("sqlite:"))
        .unwrap_or(url);

    let conn = if path == ":memory:" || path.is_empty() {
        rusqlite::Connection::open_in_memory()
    } else {
        rusqlite::Connection::open(path)
    };

    conn.map_err(|e| {
        CliError(format!(
            "failed to open SQLite database at `{}`: {e}",
            redact_database_url(url)
        ))
    })
}

/// Connects to PostgreSQL from a connection URL.
fn connect_postgres(url: &str) -> Result<postgres::Client, CliError> {
    postgres::Client::connect(url, postgres::NoTls).map_err(|e| {
        CliError(format!(
            "failed to connect to PostgreSQL at `{}`: {e}",
            redact_database_url(url)
        ))
    })
}

fn write_file(path: &Path, contents: &str) -> Result<(), CliError> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(|e| {
                CliError(format!(
                    "failed to create directory `{}`: {e}",
                    parent.display()
                ))
            })?;
        }
    }
    std::fs::write(path, contents)
        .map_err(|e| CliError(format!("failed to write `{}`: {e}", path.display())))
}

fn apply_migrations<B: MigrationBackend>(
    backend: &mut B,
    migrations_dir: &Path,
) -> Result<(), CliError> {
    let migrations = load_migrations_from_dir(migrations_dir).map_err(cli_error)?;
    let report = Migrator::new()
        .migrate(backend, &migrations)
        .map_err(cli_error)?;

    println!(
        "Applied {} migration(s) in {} ms",
        report.applied_count, report.total_time_ms
    );
    for script in &report.applied_scripts {
        println!("  - {script}");
    }
    Ok(())
}

fn report_status<B: MigrationBackend>(
    backend: &mut B,
    migrations_dir: &Path,
) -> Result<(), CliError> {
    let migrations = load_migrations_from_dir(migrations_dir).map_err(cli_error)?;
    let statuses = Migrator::new()
        .status(backend, &migrations)
        .map_err(cli_error)?;

    if statuses.is_empty() {
        println!("No migrations found in {}", migrations_dir.display());
        return Ok(());
    }

    println!(
        "{:<40} {:<12} {:<10} STATUS",
        "SCRIPT", "KIND", "VERSION"
    );
    for status in &statuses {
        let state = match status.status {
            MigrationStatusKind::Pending => "Pending",
            MigrationStatusKind::Applied => "Applied",
            MigrationStatusKind::Modified => "Modified",
            MigrationStatusKind::IgnoredBaseline => "IgnoredBaseline",
            MigrationStatusKind::Failed => "Failed",
        };
        let version = status
            .version
            .as_ref()
            .map(ToString::to_string)
            .unwrap_or_default();
        println!(
            "{:<40} {:<12} {:<10} {}",
            status.script,
            status.kind.as_str(),
            version,
            state
        );
    }
    Ok(())
}

fn repair_history<B: MigrationBackend>(
    backend: &mut B,
    migrations_dir: &Path,
) -> Result<(), CliError> {
    let migrations = load_migrations_from_dir(migrations_dir).map_err(cli_error)?;
    let report = Migrator::new()
        .repair(backend, &migrations)
        .map_err(cli_error)?;

    println!(
        "Removed {} failed entr(ies), updated {} checksum(s)",
        report.removed_failed_entries,
        report.updated_checksums.len()
    );
    for script in &report.updated_checksums {
        println!("  - {script}");
    }
    Ok(())
}

fn baseline_database<B: MigrationBackend>(
    backend: &mut B,
    version: MigrationVersion,
    description: &str,
) -> Result<(), CliError> {
    let entry = Migrator::new()
        .baseline(backend, version, description)
        .map_err(cli_error)?;
    println!(
        "Baselined database at rank {} ({})",
        entry.installed_rank, entry.description
    );
    Ok(())
}

fn print_help() {
    println!(
        r#"RURQE Developer CLI

USAGE:
    cargo db-migrate [up|status|repair] [--migrations-dir <dir>]
    cargo db-baseline --version <version> [--description <desc>]
    cargo db-codegen [--output <path>] [--lock-path <path>]
    cargo db-verify [--schema-path <path>] [--lock-path <path>]

COMMANDS:
    migrate up       Apply all pending migrations
    migrate status   Inspect status of migrations against history table
    migrate repair   Fix checksums and purge failed migration attempts
    baseline         Mark current schema as baseline without running past migrations
    codegen          Introspect database, generate Rust schema, and write schema.lock
    verify           Verify bit-for-bit parity between live database and schema.lock
"#
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_migrate_up() {
        let cmd = parse_args(&["rurqe".into(), "migrate".into(), "up".into()]).unwrap();
        assert_eq!(
            cmd,
            CliCommand::MigrateUp {
                migrations_dir: PathBuf::from("migrations"),
                database_url: None,
            }
        );
    }

    #[test]
    fn parses_baseline_command() {
        let cmd = parse_args(&[
            "rurqe".into(),
            "baseline".into(),
            "--version".into(),
            "1.0.0".into(),
            "--description".into(),
            "initial".into(),
        ])
        .unwrap();

        assert_eq!(
            cmd,
            CliCommand::Baseline {
                version: "1.0.0".parse().unwrap(),
                description: "initial".into(),
                database_url: None,
            }
        );
    }

    #[test]
    fn parses_cargo_db_migrate_invocation() {
        let cmd = parse_args(&["cargo-db-migrate".into(), "db-migrate".into()]).unwrap();
        assert_eq!(
            cmd,
            CliCommand::MigrateUp {
                migrations_dir: PathBuf::from("migrations"),
                database_url: None,
            }
        );
    }

    #[test]
    fn parses_cargo_db_codegen_invocation() {
        let cmd = parse_args(&[
            "cargo-db-codegen".into(),
            "db-codegen".into(),
            "--output".into(),
            "src/db/schema.rs".into(),
        ])
        .unwrap();
        assert_eq!(
            cmd,
            CliCommand::Codegen {
                database_url: None,
                output: PathBuf::from("src/db/schema.rs"),
                lock_path: PathBuf::from("schema.lock"),
            }
        );
    }

    #[test]
    fn parses_cargo_db_verify_invocation() {
        let cmd = parse_args(&["cargo-db-verify".into(), "db-verify".into()]).unwrap();
        assert_eq!(
            cmd,
            CliCommand::Verify {
                database_url: None,
                schema_path: None,
                lock_path: PathBuf::from("schema.lock"),
            }
        );
    }

    #[test]
    fn parses_explicit_verify_schema_path() {
        let cmd = parse_args(&[
            "cargo-db-verify".into(),
            "--schema-path".into(),
            "custom/schema.rs".into(),
        ])
        .unwrap();
        assert_eq!(
            cmd,
            CliCommand::Verify {
                database_url: None,
                schema_path: Some(PathBuf::from("custom/schema.rs")),
                lock_path: PathBuf::from("schema.lock"),
            }
        );
    }

    #[test]
    fn parses_cargo_db_baseline_invocation() {
        let cmd = parse_args(&[
            "cargo-db-baseline".into(),
            "db-baseline".into(),
            "--version".into(),
            "1.0.0".into(),
        ])
        .unwrap();
        assert_eq!(
            cmd,
            CliCommand::Baseline {
                version: "1.0.0".parse().unwrap(),
                description: "baseline".into(),
                database_url: None,
            }
        );
    }

    #[test]
    fn redacts_credentials_from_connection_urls() {
        assert_eq!(
            redact_database_url("postgres://app:secret@db.internal:5432/app"),
            "postgres://***@db.internal:5432/app"
        );
        assert_eq!(
            redact_database_url("postgresql://app:pa%40ss@db/app?sslmode=require"),
            "postgresql://***@db/app?sslmode=require"
        );
        assert_eq!(
            redact_database_url("postgres://db.internal:5432/app"),
            "postgres://db.internal:5432/app"
        );
        assert_eq!(
            redact_database_url("sqlite:/tmp/data@host.db"),
            "sqlite:/tmp/data@host.db"
        );
        assert_eq!(redact_database_url("app.db"), "app.db");
    }

    #[test]
    fn parses_codegen_command() {
        let cmd = parse_args(&[
            "cargo-db-codegen".into(),
            "--output".into(),
            "src/db/schema.rs".into(),
        ])
        .unwrap();

        assert_eq!(
            cmd,
            CliCommand::Codegen {
                database_url: None,
                output: PathBuf::from("src/db/schema.rs"),
                lock_path: PathBuf::from("schema.lock"),
            }
        );
    }
}
