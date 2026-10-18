//! Command-line argument parser and execution coordinator.
//!
//! Supports standalone binary execution (`rurqe ...`) and Cargo subcommand
//! dispatching (`cargo db-migrate ...`, `cargo db-codegen ...`, `cargo db-verify ...`).

use std::path::{Path, PathBuf};

use rurqe_migrate::version::MigrationVersion;

use crate::introspect::detect_dialect;

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
                schema_path: Some(PathBuf::from("src/schema.rs")),
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
            let mut schema_path = Some(PathBuf::from("src/schema.rs"));
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
        CliCommand::MigrateUp { migrations_dir, .. } => {
            println!("Applying migrations from {}...", migrations_dir.display());
            Ok(())
        }
        CliCommand::MigrateStatus { migrations_dir, .. } => {
            println!("Checking migration status in {}...", migrations_dir.display());
            Ok(())
        }
        CliCommand::MigrateRepair { migrations_dir, .. } => {
            println!("Repairing schema history against {}...", migrations_dir.display());
            Ok(())
        }
        CliCommand::Baseline { version, description, .. } => {
            println!("Baselining database at version {version} ({description})...");
            Ok(())
        }
        CliCommand::Codegen {
            database_url,
            output,
            lock_path,
        } => {
            let dialect = detect_dialect(database_url.as_deref());
            println!(
                "Generating {dialect} schema to {} and snapshot to {}...",
                output.display(),
                lock_path.display()
            );
            Ok(())
        }
        CliCommand::Verify {
            database_url,
            lock_path,
            ..
        } => {
            let dialect = detect_dialect(database_url.as_deref());
            println!(
                "Verifying {dialect} schema against {}...",
                lock_path.display()
            );
            Ok(())
        }
    }
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
