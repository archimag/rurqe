//! Flyway-compatible migration model for RURQE.
//!
//! Migrations are plain SQL scripts named either `V{version}__{description}.sql`
//! (versioned, applied once in ascending version order) or
//! `R__{description}.sql` (repeatable, re-applied whenever the script checksum
//! changes). Every script is identified by its SHA-256 [`Checksum`], so a
//! retroactive edit of an already applied migration is detected instead of
//! silently diverging from the database state.
//!
//! Applied migrations are recorded as [`SchemaHistoryEntry`] rows of the
//! [`SCHEMA_HISTORY_TABLE`] metadata table.
//!
//! ```
//! use rurqe_migrate::{Migration, MigrationKind};
//!
//! let migration = Migration::parse("V2_1__add_orders.sql", "CREATE TABLE orders ();")
//!     .expect("valid script name");
//!
//! assert_eq!(migration.kind, MigrationKind::Versioned);
//! assert_eq!(migration.version.as_ref().map(ToString::to_string).as_deref(), Some("2.1"));
//! assert_eq!(migration.description, "add orders");
//! assert_eq!(migration.checksum.to_hex().len(), 64);
//! ```

pub mod catalog;
pub mod checkpoint;
pub mod checksum;
pub mod embedded;
pub mod error;
pub mod history;
pub mod migration;
pub mod runner;
pub mod version;

pub use catalog::{ColumnDef, ForeignKeyDef, SchemaCatalog, TableDef, TypeKind};
pub use checkpoint::{Checkpoint, CHECKPOINT_FILE};
pub use checksum::{sha256, Checksum};
pub use embedded::EmbeddedMigrations;
pub use error::Error;
pub use history::{create_schema_history_table_sql, SchemaHistoryEntry, SCHEMA_HISTORY_TABLE};
pub use migration::{Migration, MigrationKind};
pub use runner::{
    InMemoryBackend, InMemoryTransaction, MigrationBackend, MigrationReport, MigrationStatus,
    MigrationStatusKind, MigrationTransaction, Migrator, RepairReport,
};
pub use version::MigrationVersion;
