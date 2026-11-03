//! Command-line argument parser and execution coordinator.
//!
//! Supports standalone binary execution (`rurqe ...`) and Cargo subcommand
//! dispatching (`cargo db-migrate ...`, `cargo db-codegen ...`, `cargo db-verify ...`).
//!
//! The module is split into focused submodules: `args` parses command-line
//! tokens into typed [`CliCommand`] values, `redact` masks the credentials a
//! connection string carries, `connection` owns the database connection
//! lifecycle, and `commands` implements the behaviour of every CLI command. This
//! facade keeps only the command dispatch in [`run_cli`], while the `error`
//! module owns the failure contract every command reports through.

mod args;
mod commands;
mod connection;
pub mod error;
mod redact;
mod report;

use std::path::{Path, PathBuf};
use std::time::Duration;

#[cfg(test)]
mod tests;

pub use args::{parse_args, CliCommand};
pub(crate) use args::{resolve_cleanup_url, resolve_equation_url};
pub use error::{CliCause, CliError};
pub(crate) use commands::best_effort;
pub(crate) use connection::{connect, Connection};
pub use redact::redact_database_url;
pub use report::CommandOutput;

use self::commands::{
    apply_migrations, baseline_database, clean_database, cleanup_stage, commit_stage, help_text,
    initialise_stage, prune_migrations, repair_history, report_status, verify_stage, write_file,
    Project,
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
/// The configuration path is the fallback of the cascade rather than a constant
/// of this function, so whoever owns the directory holding `rurqe.toml` decides
/// which file a command reads when the invocation names none: a test points every
/// command it runs at a file it wrote into a private temporary directory, and an
/// embedding application keeps two projects apart. An explicit `--config` of the
/// invocation wins over it, because the operator named that file directly.
/// [`run_cli`] is the entry point that passes the conventional `rurqe.toml` of the
/// working directory.
///
/// Every artifact path of a command is resolved through the loaded project, so the
/// file is read once per invocation and the cascade of every path — the explicit
/// flag, then the declared key, then the conventional layout — starts from the same
/// declaration. `stage cleanup` consumes no artifact path, but it accepts
/// `--config`, so a configuration it was pointed at is validated exactly like on the
/// commands that do resolve paths.
pub(crate) fn run_cli_with_config(
    args: Vec<String>,
    config_path: &Path,
) -> Result<CommandOutput, CliError> {
    let cmd = parse_args(&args)?;

    // Help and version describe the toolchain rather than one project, so they are
    // answered through their own arms below without a configuration being read: a
    // malformed `rurqe.toml` never suppresses the help of the binary that reports
    // it. Every other action resolves its paths through the loaded project, so the
    // file is read once per command.
    let load_project = |config: Option<PathBuf>| Project::load(config.as_deref(), config_path);

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
            config,
            database_url,
            schema,
        } => {
            let project = load_project(config)?;
            // The tail and the boundary marker carry no flag of their own: the
            // configuration declares them, and the `rurqe/` convention closes the
            // cascade when it declares neither.
            let migrations_dir = project.migrations_dir();
            let checkpoint_path = project.checkpoint_path();
            let selection = project.selection(schema.as_deref());
            with_migration_backend!(database_url, selection, |backend| {
                apply_migrations(&mut backend, &migrations_dir, &checkpoint_path)
            })
        }
        CliCommand::MigrateStatus {
            config,
            database_url,
            schema,
        } => {
            let project = load_project(config)?;
            let migrations_dir = project.migrations_dir();
            let checkpoint_path = project.checkpoint_path();
            let selection = project.selection(schema.as_deref());
            with_migration_backend!(database_url, selection, |backend| {
                report_status(&mut backend, &migrations_dir, &checkpoint_path)
            })
        }
        CliCommand::MigrateRepair {
            config,
            database_url,
            schema,
        } => {
            let project = load_project(config)?;
            let migrations_dir = project.migrations_dir();
            let checkpoint_path = project.checkpoint_path();
            let selection = project.selection(schema.as_deref());
            with_migration_backend!(database_url, selection, |backend| {
                repair_history(&mut backend, &migrations_dir, &checkpoint_path)
            })
        }
        CliCommand::MigrateClean {
            config,
            database_url,
            schema,
            force,
        } => {
            let project = load_project(config)?;
            let selection = project.selection(schema.as_deref());
            // The refusal and the report name the target, so the URL is resolved
            // and redacted once here instead of being echoed from the invocation.
            let target = redact_database_url(&resolve_database_url(database_url.clone())?);
            with_migration_backend!(database_url, selection, |backend| {
                clean_database(&mut backend, &target, force)
            })
        }
        CliCommand::Baseline {
            config,
            version,
            description,
            database_url,
        } => {
            // The command records a marker and consumes no artifact path, but a
            // configuration it accepted is validated like on every other command.
            load_project(config)?;
            with_migration_backend!(database_url, |backend| {
                baseline_database(&mut backend, version, &description)
            })
        }
        CliCommand::Codegen {
            config,
            database_url,
            output,
            lock_path,
            domain_types,
            schema,
        } => {
            let project = load_project(config)?;
            let selection = project.selection(schema.as_deref())?;
            let url = resolve_database_url(database_url)?;
            let (catalog, notice) = introspect_database_selection_notice(&url, &selection)?;
            let output = project.codegen_output(output);
            let lock_path = project.lock_path(lock_path);
            let mut options = project.codegen_options();
            if domain_types {
                options.domain_types = true;
            }
            let code = generate_schema_rs(&catalog, &options)
                .map_err(|err| CliError::message(format!("failed to generate schema code: {err}")))?;
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
            config,
            database_url,
            schema_path,
            lock_path,
            schema,
        } => {
            let project = load_project(config)?;
            let selection = project.selection(schema.as_deref())?;
            let lock_path = project.lock_path(lock_path);
            let url = resolve_database_url(database_url)?;
            let expected_lock = std::fs::read_to_string(&lock_path).map_err(|e| {
                CliError::message(format!(
                    "failed to read schema lock `{}`: {e}",
                    lock_path.display()
                ))
            })?;
            let schema_code = project.resolve_schema_code(schema_path)?;
            let (catalog, notice) = introspect_database_selection_notice(&url, &selection)?;
            verify_drift(&catalog, &expected_lock, schema_code.as_deref())
                .map_err(|err| CliError::with_cause(CliCause::Drift(err)))?;

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
            config,
            stage_dir,
            version,
            description,
            database_url,
            schema,
        } => {
            let project = load_project(config)?;
            let stage_dir = project.stage_dir(stage_dir);
            let selection = project.selection(schema.as_deref());
            with_migration_backend!(database_url, selection, |backend| {
                initialise_stage(&mut backend, &stage_dir, version, &description)
            })
        }
        CliCommand::StageVerify {
            config,
            stage_dir,
            base_stage_dir,
            migrations_dir,
            checkpoint_path,
            scratch_url,
            database_url,
            schema,
        } => {
            // The equation materialises both sides in a scratch database and reads
            // their catalogs over the selected schemas, so the selection is resolved
            // before that database is opened: a project keeping its relations
            // outside `public` — or declaring secondary schemas — describes what it
            // declares instead of an empty `public`, and a malformed configuration
            // aborts the verification exactly like every other catalog-digest command.
            let project = load_project(config)?;
            let selection = project.selection(schema.as_deref())?;
            let stage_dir = project.stage_dir(stage_dir);
            let migrations_dir = migrations_dir.unwrap_or_else(|| project.migrations_dir());
            let checkpoint_path = checkpoint_path.unwrap_or_else(|| project.checkpoint_path());
            let url = resolve_equation_url(scratch_url.or(database_url))?;
            verify_stage(
                &stage_dir,
                base_stage_dir.as_deref(),
                &migrations_dir,
                &checkpoint_path,
                &selection,
                &url,
            )
        }
        CliCommand::StageCommit {
            config,
            version,
            database_url,
            scratch_url,
            stage_dir,
            lock_path,
            checkpoint_path,
            schema,
        } => {
            let project = load_project(config)?;
            let selection = project.selection(schema.as_deref())?;
            let url = resolve_database_url(database_url)?;
            let checkpoint_path = checkpoint_path.unwrap_or_else(|| project.checkpoint_path());
            commit_stage(
                version,
                &url,
                scratch_url.as_deref(),
                &selection,
                &project.stage_dir(stage_dir),
                &project.lock_path(lock_path),
                &checkpoint_path,
            )
        }
        CliCommand::StageCleanup {
            config,
            scratch_url,
            database_url,
            older_than_secs,
            force,
        } => {
            // The sweep consumes no artifact path, but it accepts `--config`, so
            // the file is validated strictly like on every other command.
            load_project(config)?;
            let url = resolve_cleanup_url(scratch_url.or(database_url))?;
            cleanup_stage(&url, Duration::from_secs(older_than_secs), force)
        }
        CliCommand::Prune {
            config,
            before,
            migrations_dir,
            checkpoint_path,
            dry_run,
        } => {
            let project = load_project(config)?;
            let migrations_dir = migrations_dir.unwrap_or_else(|| project.migrations_dir());
            let checkpoint_path = checkpoint_path.unwrap_or_else(|| project.checkpoint_path());
            prune_migrations(before, &migrations_dir, &checkpoint_path, dry_run)
        }
    }
}
