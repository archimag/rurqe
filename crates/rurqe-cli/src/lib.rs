//! Developer toolchain for live catalog introspection, code generation,
//! migration execution, and CI schema drift verification (`schema.lock`).

pub mod cli;
pub mod codegen;
pub mod config;
pub mod driver;
pub mod drift;
pub mod introspect;
pub mod loader;
pub mod stage;
pub mod target;

pub use cli::{parse_args, run_cli, CliCommand, CliError};
pub use codegen::{generate_schema_rs, CodegenError, CodegenOptions};
pub use config::{CodegenConfig, ConfigError};
pub use drift::{verify_drift, DiffWording, DriftError, DriftReport, SchemaDiff};
pub use driver::{PostgresMigrationBackend, SqliteMigrationBackend};
pub use introspect::{
    catalog_from_metadata, catalog_from_sqlite_metadata, detect_dialect, map_sql_type_to_kind,
    DatabaseDialect, IntrospectQueries, RawColumnMetadata, RawForeignKeyMetadata,
    RawIndexMetadata, RawPrimaryKeyMetadata, RawSqliteColumnMetadata, RawSqliteForeignKeyMetadata,
    RawSqliteTableMetadata, RawTableMetadata,
};
pub use loader::{load_migrations_from_dir, read_migration_files};
pub use stage::{GoldenEquation, GoldenReport, StageError};

/// Main CLI execution entry point for standalone and Cargo subcommand binaries.
pub fn run() {
    let args: Vec<String> = std::env::args().collect();
    if let Err(err) = run_cli(args) {
        eprintln!("Error: {err}");
        std::process::exit(1);
    }
}
