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

/// Main CLI execution entry point for standalone and Cargo subcommand binaries.
///
/// The report of a command is printed here, at the process boundary, so the
/// command implementations stay callable as a library without writing to a
/// stream of their own.
pub fn run() {
    let args: Vec<String> = std::env::args().collect();
    match run_cli(args) {
        Ok(output) => output.print(),
        Err(err) => {
            eprintln!("Error: {err}");
            std::process::exit(1);
        }
    }
}
