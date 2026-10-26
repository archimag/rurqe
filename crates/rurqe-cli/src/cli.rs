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
mod report;

use std::time::Duration;

#[cfg(test)]
mod tests;

pub use args::{parse_args, CliCommand, CliError};
pub(crate) use args::{resolve_cleanup_url, resolve_equation_url};
pub(crate) use connection::{connect, Connection};
pub use redact::redact_database_url;
pub use report::CommandOutput;

use self::commands::{
    apply_migrations, baseline_database, cleanup_stage, codegen_options, commit_stage, help_text,
    initialise_stage, load_config, prune_migrations, repair_history, report_status, resolve_schema,
    resolve_schema_code, schema_of, verify_stage, write_file,
};
use self::connection::resolve_database_url;

use crate::codegen::generate_schema_rs;
use crate::drift::verify_drift;
use crate::driver::{PostgresMigrationBackend, SqliteMigrationBackend};
use crate::introspect::{introspect_database_schema_notice, report_vacuous_postgres_schema};

/// Opens the migration backend for `database_url` and evaluates `$run` against it.
///
/// SQLite and PostgreSQL backends are distinct static types, so the backend is
/// constructed once per command through the shared connection helper instead of
/// repeating the dialect match and connection logic by hand.
///
/// `$schema` is a `Result` the PostgreSQL arm alone evaluates: a SQLite database
/// holds one unnamed catalog, so the selection never applies to it and a project
/// migrating SQLite is never made to load the selection configuration. The
/// resolved name reaches `PostgresMigrationBackend::with_schema`, so the recorded
/// catalog digest — and the checkpoint guard comparing it — describes the same
/// schema the codegen snapshot and the drift gate read.
///
/// The two-argument form serves the one command that records no catalog digest:
/// `baseline` only writes a history marker, so it carries no schema selection and
/// keeps the backend on the default schema.
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
    ($database_url:expr, $schema:expr, |$backend:ident| $run:expr) => {{
        let url = resolve_database_url($database_url)?;
        match connect(&url)? {
            Connection::Sqlite(conn) => {
                let mut $backend = SqliteMigrationBackend::new(conn);
                $run
            }
            Connection::Postgres(mut client) => {
                let schema = $schema?;
                let notice = report_vacuous_postgres_schema(&mut client, &schema)?;
                let mut $backend = PostgresMigrationBackend::new(client).with_schema(schema);

                // The notice travels in the report of the command, so a digest
                // computed over a schema that describes nothing reaches the
                // operator whether or not a tracing sink is installed.
                let mut output = $run?;
                output.notice(notice);
                Ok(output)
            }
        }
    }};
}

/// Runs the CLI toolchain and returns the report it produced.
///
/// The command implementations of this crate are reachable as a library, so
/// none of them writes to a stream of its own: the report comes back to the
/// caller and the process entry point [`run`](crate::run) prints it. Warnings
/// about the environment travel through the `tracing` facade instead, which
/// leaves the sink to the application that embeds the crate.
pub fn run_cli(args: Vec<String>) -> Result<CommandOutput, CliError> {
    let cmd = parse_args(&args)?;

    match cmd {
        CliCommand::Help => {
            let mut output = CommandOutput::new();
            output.text(help_text());
            Ok(output)
        }
        CliCommand::Version => Ok(CommandOutput::single(format!(
            "rurqe-cli {}",
            env!("CARGO_PKG_VERSION")
        ))),
        CliCommand::MigrateUp {
            migrations_dir,
            checkpoint_path,
            database_url,
            schema,
        } => with_migration_backend!(database_url, resolve_schema(schema.as_deref()), |backend| {
            apply_migrations(&mut backend, &migrations_dir, &checkpoint_path)
        }),
        CliCommand::MigrateStatus {
            migrations_dir,
            database_url,
            schema,
        } => with_migration_backend!(database_url, resolve_schema(schema.as_deref()), |backend| {
            report_status(&mut backend, &migrations_dir)
        }),
        CliCommand::MigrateRepair {
            migrations_dir,
            database_url,
            schema,
        } => with_migration_backend!(database_url, resolve_schema(schema.as_deref()), |backend| {
            repair_history(&mut backend, &migrations_dir)
        }),
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
            schema,
        } => {
            let config = load_config()?;
            let schema = schema_of(schema.as_deref(), &config);
            let url = resolve_database_url(database_url)?;
            let (catalog, notice) = introspect_database_schema_notice(&url, &schema)?;
            let mut options = codegen_options(config);
            if domain_types {
                options.domain_types = true;
            }
            let code = generate_schema_rs(&catalog, &options)
                .map_err(|err| CliError(format!("failed to generate schema code: {err}")))?;
            write_file(&output, &code)?;
            write_file(&lock_path, &catalog.to_lock())?;

            let mut report = CommandOutput::new();
            report.notice(notice);
            report.line(format!(
                "Generated schema to {} and snapshot to {} (hash {})",
                output.display(),
                lock_path.display(),
                catalog.schema_hash()
            ));
            Ok(report)
        }
        CliCommand::Verify {
            database_url,
            schema_path,
            lock_path,
            schema,
        } => {
            let schema = resolve_schema(schema.as_deref())?;
            let url = resolve_database_url(database_url)?;
            let expected_lock = std::fs::read_to_string(&lock_path).map_err(|e| {
                CliError(format!(
                    "failed to read schema lock `{}`: {e}",
                    lock_path.display()
                ))
            })?;
            let schema_code = resolve_schema_code(schema_path)?;
            let (catalog, notice) = introspect_database_schema_notice(&url, &schema)?;
            verify_drift(&catalog, &expected_lock, schema_code.as_deref())
                .map_err(|err| CliError(err.to_string()))?;

            // A selection that describes nothing compares nothing against
            // nothing, so the pass is reported together with the notice that
            // says what the digest really covered.
            let mut report = CommandOutput::new();
            report.notice(notice);
            report.line(format!(
                "Schema verification passed against {}",
                lock_path.display()
            ));
            Ok(report)
        }
        CliCommand::StageInit {
            stage_dir,
            version,
            description,
            database_url,
            schema,
        } => with_migration_backend!(database_url, resolve_schema(schema.as_deref()), |backend| {
            initialise_stage(&mut backend, &stage_dir, version, &description)
        }),
        CliCommand::StageVerify {
            stage_dir,
            base_stage_dir,
            migrations_dir,
            checkpoint_path,
            scratch_url,
            database_url,
        } => {
            let url = resolve_equation_url(scratch_url.or(database_url))?;
            verify_stage(
                &stage_dir,
                base_stage_dir.as_deref(),
                &migrations_dir,
                &checkpoint_path,
                &url,
            )
        }
        CliCommand::StageCommit {
            version,
            database_url,
            stage_dir,
            lock_path,
            checkpoint_path,
            schema,
        } => {
            let schema = resolve_schema(schema.as_deref())?;
            let url = resolve_database_url(database_url)?;
            commit_stage(
                version,
                &url,
                &schema,
                &stage_dir,
                &lock_path,
                &checkpoint_path,
            )
        }
        CliCommand::StageCleanup {
            scratch_url,
            database_url,
            older_than_secs,
            force,
        } => {
            let url = resolve_cleanup_url(scratch_url.or(database_url))?;
            cleanup_stage(&url, Duration::from_secs(older_than_secs), force)
        }
        CliCommand::Prune {
            before,
            migrations_dir,
            checkpoint_path,
            dry_run,
        } => prune_migrations(before, &migrations_dir, &checkpoint_path, dry_run),
    }
}
