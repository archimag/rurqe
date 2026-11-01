//! Engine-neutral vocabulary of catalog introspection.
//!
//! An engine reports its catalog as plain rows — one row per table, column, enum
//! variant, key, index, check, view, or trigger — and the catalog those rows
//! describe is assembled here, independently of the driver that read them. The
//! module is the single home of that vocabulary in the workspace: the PostgreSQL
//! and SQLite adapters of the toolchain implement the catalog statements and hand
//! the rows over, while the assembly, the SQL type mapping, and the schema
//! selection live here, so a driver crate depends on the migration crate alone.
//!
//! [`SchemaSelection`] travels with the vocabulary because every catalog digest
//! describes a selection of schemas, and an engine that computes such a digest
//! must name the same selection the toolchain reads.

mod assembly;
mod metadata;
mod selection;
mod type_mapping;

pub use assembly::{catalog_from_metadata, catalog_from_metadata_with_sections, fold_indexes};
pub use metadata::{
    parse_foreign_key_action, RawCatalogSections, RawCheckMetadata, RawColumnMetadata,
    RawEnumMetadata, RawForeignKeyMetadata, RawIndexMetadata, RawPrimaryKeyMetadata,
    RawTableMetadata, RawTriggerMetadata, RawViewMetadata, TableKey,
};
pub use selection::SchemaSelection;
pub use type_mapping::{map_sql_type_to_kind, map_sqlite_declared_type_to_kind};
