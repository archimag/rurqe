//! Command-line argument parser and execution coordinator.
//!
//! Supports standalone binary execution (`rurqe ...`) and Cargo subcommand
//! dispatching (`cargo db-migrate ...`, `cargo db-codegen ...`, `cargo db-verify ...`).
//!
//! The module is split into focused submodules: `args` parses command-line
//! tokens into typed [`CliCommand`] values and redacts credentials from
//! connection URLs, `connection` owns the database connection lifecycle, and
//! `commands` implements the behaviour of every CLI command. This facade keeps
//! only the command dispatch in [`run_cli`].

mod args;
mod commands;
mod connection;

#[cfg(test)]
mod tests;

pub use args::{parse_args, redact_database_url, CliCommand, CliError};
pub(crate) use connection::{connect, Connection};

use self::commands::{
    apply_migrations, baseline_database, print_help, repair_history, report_status,
    resolve_schema_code, write_file,
};
use self::connection::resolve_database_url;

use crate::codegen::{generate_schema_rs, CodegenOptions};
use crate::drift::verify_drift;
use crate::driver::{PostgresMigrationBackend, SqliteMigrationBackend};
use crate::introspect::introspect_database;

/// Opens the migration backend for `database_url` and evaluates `$run` against it.
///
/// SQLite and PostgreSQL backends are distinct static types, so the backend is
/// constructed once per command through the shared connection helper instead of
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
