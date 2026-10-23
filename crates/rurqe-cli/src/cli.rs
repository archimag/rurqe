//! Command-line argument parser and execution coordinator.
//!
//! Supports standalone binary execution (`rurqe ...`) and Cargo subcommand
//! dispatching (`cargo db-migrate ...`, `cargo db-codegen ...`, `cargo db-verify ...`).
//!
//! The module is split into focused submodules: `args` parses command-line
//! tokens into typed [`CliCommand`] values, `redact` masks the credentials a
//! connection string carries, `connection` owns the database connection
//! lifecycle, and `commands` implements the behaviour of every CLI command. This
//! facade keeps only the command dispatch in [`run_cli`].

mod args;
mod commands;
mod connection;
mod redact;

#[cfg(test)]
mod tests;

pub use args::{parse_args, CliCommand, CliError};
pub use redact::redact_database_url;
pub(crate) use args::resolve_equation_url;
pub(crate) use connection::{connect, Connection};

use self::commands::{
    apply_migrations, baseline_database, codegen_options, commit_stage, initialise_stage,
    print_help, prune_migrations, repair_history, report_status, resolve_schema_code,
    verify_stage, write_file,
};
use self::connection::resolve_database_url;

use crate::codegen::generate_schema_rs;
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
            checkpoint_path,
            database_url,
        } => {
            with_migration_backend!(database_url, |backend| {
                apply_migrations(&mut backend, &migrations_dir, &checkpoint_path)
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
            domain_types,
        } => {
            let url = resolve_database_url(database_url)?;
            let catalog = introspect_database(&url)?;
            let mut options = codegen_options()?;
            if domain_types {
                options.domain_types = true;
            }
            let code = generate_schema_rs(&catalog, &options)
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
        CliCommand::StageInit {
            stage_dir,
            version,
            description,
            database_url,
        } => {
            with_migration_backend!(database_url, |backend| {
                initialise_stage(&mut backend, &stage_dir, version, &description)
            })
        }
        CliCommand::StageVerify {
            stage_dir,
            base_stage_dir,
            migrations_dir,
            scratch_url,
            database_url,
        } => {
            let url = resolve_equation_url(scratch_url.or(database_url))?;
            verify_stage(
                &stage_dir,
                base_stage_dir.as_deref(),
                &migrations_dir,
                &url,
            )
        }
        CliCommand::StageCommit {
            version,
            database_url,
            lock_path,
            checkpoint_path,
        } => {
            let url = resolve_database_url(database_url)?;
            commit_stage(version, &url, &lock_path, &checkpoint_path)
        }
        CliCommand::Prune {
            before,
            migrations_dir,
            checkpoint_path,
            dry_run,
        } => prune_migrations(before, &migrations_dir, &checkpoint_path, dry_run),
    }
}
