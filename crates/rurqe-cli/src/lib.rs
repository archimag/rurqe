//! Developer toolchain for live catalog introspection, code generation,
//! migration execution, and CI schema drift verification (`schema.lock`).

pub mod cli;
pub mod codegen;
pub mod drift;
pub mod introspect;
pub mod stage;

// Modules below are implementation details reached through the facade of this
// crate root, so they stay private: only the items re-exported below are part of
// the published surface.
mod config;
mod driver;
mod loader;
mod target;

pub use cli::{parse_args, run_cli, CliCommand, CliError, CommandOutput};
pub use codegen::{generate_schema_rs, CodegenError, CodegenOptions};
pub use config::{CodegenConfig, ConfigError};
pub use drift::{verify_drift, DiffWording, DriftError, DriftReport, SchemaDiff};
pub use driver::{PostgresMigrationBackend, SqliteMigrationBackend};
pub use introspect::{
    catalog_from_metadata, catalog_from_sqlite_metadata, detect_dialect, map_sql_type_to_kind,
    DatabaseDialect, RawColumnMetadata, RawForeignKeyMetadata, RawIndexMetadata,
    RawPrimaryKeyMetadata, RawSqliteColumnMetadata, RawSqliteForeignKeyMetadata,
    RawSqliteTableMetadata, RawTableMetadata,
};
pub use loader::{load_migrations_from_dir, read_migration_files};
pub use stage::{GoldenEquation, GoldenReport, StageError};

/// Installs the diagnostic sink the shipped binaries report their notices through.
///
/// Every environment diagnostic of this crate — a schema selection that describes
/// nothing, an accepted cleartext session, a boundary whose directory could not be
/// flushed — travels through the `tracing` facade, which leaves the sink to the
/// application that embeds the crate. The shipped binaries are that application,
/// so the sink is installed at their single entry point: without it the global
/// dispatcher stays the no-op default and every notice is dropped before it
/// reaches the operator.
///
/// The sink writes to standard error at `WARN` and above, so the report of a
/// command on standard output stays machine-readable. It is installed once per
/// process and never replaces a sink an embedding application installed first,
/// which keeps [`run_cli`] usable as a plain library call.
pub fn install_diagnostics() {
    static INSTALLED: std::sync::Once = std::sync::Once::new();

    INSTALLED.call_once(|| {
        let sink = tracing_subscriber::fmt::Subscriber::builder()
            .compact()
            .with_max_level(tracing::Level::WARN)
            .with_writer(std::io::stderr)
            .with_target(false)
            .finish();

        // An embedding application that installed its own sink keeps it: its
        // notices are already routed where it wants them.
        let _ = tracing::subscriber::set_global_default(sink);
    });
}

/// Main CLI execution entry point for standalone and Cargo subcommand binaries.
///
/// The report of a command is printed here, at the process boundary, so the
/// command implementations stay callable as a library without writing to a
/// stream of their own. The diagnostic sink the notices of the crate travel
/// through is installed here for the same reason: the binary is the application
/// the `tracing` facade delegates the sink to.
pub fn run() {
    install_diagnostics();

    let args: Vec<String> = std::env::args().collect();
    match run_cli(args) {
        Ok(output) => output.print(),
        Err(err) => {
            eprintln!("Error: {err}");
            std::process::exit(1);
        }
    }
}
