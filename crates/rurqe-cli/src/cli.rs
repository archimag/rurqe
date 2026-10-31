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

use std::path::Path;
use std::time::Duration;

#[cfg(test)]
mod tests;

pub use args::{parse_args, CliCommand, CliError};
pub(crate) use args::{resolve_cleanup_url, resolve_equation_url};
pub(crate) use commands::best_effort;
pub(crate) use connection::{connect, Connection};
pub use redact::redact_database_url;
pub use report::CommandOutput;

use self::commands::{
    apply_migrations, baseline_database, cleanup_stage, codegen_options, commit_stage, help_text,
    initialise_stage, load_config, prune_migrations, repair_history, report_status, resolve_schema,
    resolve_schema_code, resolve_selection, selection_of, verify_stage, write_file,
};
use self::connection::resolve_database_url;

use crate::codegen::generate_schema_rs;
use crate::config::DEFAULT_CONFIG_PATH;
use crate::drift::verify_drift;
use crate::driver::{PostgresMigrationBackend, SqliteMigrationBackend};
use crate::introspect::{introspect_database_selection_notice, report_vacuous_postgres_selection};

/// Opens the migration backend for `database_url` and evaluates `$run` against it.
///
/// SQLite and PostgreSQL backends are distinct static types, so the backend is
/// constructed once per command through the shared connection helper instead of
/// repeating the dialect match and connection logic by hand.
///
/// `$selection` is a `Result` resolved before the engine is known, so the project
/// configuration behind it is loaded and validated for every engine: a malformed
/// `rurqe.toml` fails a SQLite command exactly like a PostgreSQL one, instead of
/// being tolerated on the engine whose backend never reads the resolved schemas.
/// Only that selection is engine-conditional — a SQLite database holds one
/// unnamed catalog the selection would never describe, so the SQLite arm drops
/// it — while the PostgreSQL arm hands it to
/// `PostgresMigrationBackend::with_schemas`, so the recorded catalog digest — and
/// the checkpoint guard comparing it — describes the same schemas the codegen
/// snapshot and the drift gate read.
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
                let mut $backend = PostgresMigrationBackend::new(*client);
                $run
            }
        }
    }};
    ($database_url:expr, $selection:expr, |$backend:ident| $run:expr) => {{
        let url = resolve_database_url($database_url)?;

        // The selection is resolved before the engine is known, so the project
        // configuration behind it is validated for a SQLite target as well: a file
        // skipped on that engine would let a typo pass on the very commands that
        // record a catalog digest. Only the resolved selection is
        // engine-conditional, because a SQLite database holds one unnamed catalog
        // the selection would never describe.
        let selection = $selection?;

        match connect(&url)? {
            Connection::Sqlite(conn) => {
                let mut $backend = SqliteMigrationBackend::new(conn);
                $run
            }
            Connection::Postgres(client) => {
                let mut client = *client;
                let notice = report_vacuous_postgres_selection(&mut client, &selection)?;
                let mut $backend = PostgresMigrationBackend::new(client).with_schemas(selection);

                // The notice travels in the report of the command, so a digest
                // computed over a selection that describes nothing reaches the
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
    run_cli_with_config(args, Path::new(DEFAULT_CONFIG_PATH))
}

/// Runs the CLI toolchain against the project configuration at `config_path`.
///
/// The configuration path is a parameter rather than a constant of this function,
/// so whoever owns the directory holding `rurqe.toml` decides which file a
/// command reads: a test points every command it runs at a file it wrote into a
/// private temporary directory, and an embedding application keeps two projects
/// apart. Neither of them has to write into the working directory of the process
/// to configure a command. [`run_cli`] is the entry point that passes the
/// conventional `rurqe.toml` of the working directory.
pub(crate) fn run_cli_with_config(
    args: Vec<String>,
    config_path: &Path,
) -> Result<CommandOutput, CliError> {
    let cmd = parse_args(&args)?;

    // Every catalog-digest arm below resolves its schemas against the
    // configuration file this run was pointed at, so the path is bound once here
    // and the arms keep naming only the schema selection. The two stage arms that
    // still thread one schema through their entry points derive it from the same
    // selection.
    let resolve_selection = |explicit: Option<&str>| resolve_selection(explicit, config_path);
    let resolve_schema = |explicit: Option<&str>| resolve_schema(explicit, config_path);

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
        } => with_migration_backend!(database_url, resolve_selection(schema.as_deref()), |backend| {
            apply_migrations(&mut backend, &migrations_dir, &checkpoint_path)
        }),
        CliCommand::MigrateStatus {
            migrations_dir,
            database_url,
            schema,
        } => with_migration_backend!(database_url, resolve_selection(schema.as_deref()), |backend| {
            report_status(&mut backend, &migrations_dir)
        }),
        CliCommand::MigrateRepair {
            migrations_dir,
            database_url,
            schema,
        } => with_migration_backend!(database_url, resolve_selection(schema.as_deref()), |backend| {
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
            let config = load_config(config_path)?;
            let selection = selection_of(schema.as_deref(), &config)?;
            let url = resolve_database_url(database_url)?;
            let (catalog, notice) = introspect_database_selection_notice(&url, &selection)?;
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
            let selection = resolve_selection(schema.as_deref())?;
            let url = resolve_database_url(database_url)?;
            let expected_lock = std::fs::read_to_string(&lock_path).map_err(|e| {
                CliError(format!(
                    "failed to read schema lock `{}`: {e}",
                    lock_path.display()
                ))
            })?;
            let schema_code = resolve_schema_code(schema_path)?;
            let (catalog, notice) = introspect_database_selection_notice(&url, &selection)?;
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
        } => with_migration_backend!(database_url, resolve_selection(schema.as_deref()), |backend| {
            initialise_stage(&mut backend, &stage_dir, version, &description)
        }),
        CliCommand::StageVerify {
            stage_dir,
            base_stage_dir,
            migrations_dir,
            checkpoint_path,
            scratch_url,
            database_url,
            schema,
        } => {
            // The equation materialises both sides in a scratch database and reads
            // their catalogs over the selected schema, so the selection is resolved
            // before that database is opened: a project keeping its relations
            // outside `public` describes the schema it declares instead of an empty
            // `public`, and a malformed configuration aborts the verification
            // exactly like every other catalog-digest command.
            let schema = resolve_schema(schema.as_deref())?;
            let url = resolve_equation_url(scratch_url.or(database_url))?;
            verify_stage(
                &stage_dir,
                base_stage_dir.as_deref(),
                &migrations_dir,
                &checkpoint_path,
                &schema,
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
