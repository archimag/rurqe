//! Command-line argument parsing and connection-URL redaction.

use std::path::{Path, PathBuf};

use rurqe_migrate::version::MigrationVersion;

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
