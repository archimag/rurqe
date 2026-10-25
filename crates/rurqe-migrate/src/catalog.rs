//! Engine-neutral schema catalog and its canonical `schema.lock` snapshot.
//!
//! The catalog is the single source of truth shared by introspection, code
//! generation, and drift verification: it records enum types, tables, columns,
//! primary keys, foreign keys, and secondary or unique indexes in an
//! engine-independent form and renders them into a deterministic snapshot —
//! the versioned TOML v4 document, while snapshots of the legacy v1–v3 text
//! formats remain readable.
//!
//! Determinism is achieved by sorting enum types, tables, columns, foreign keys,
//! and indexes by name, so two catalogs built in a different declaration order
//! still produce the very same snapshot — and therefore the very same
//! [`SchemaCatalog::schema_hash`].
//!
//! ```
//! use rurqe_migrate::{ColumnDef, SchemaCatalog, TableDef, TypeKind};
//!
//! // Columns are declared in the canonical order the snapshot sorts them
//! // into, so the parsed snapshot equals the catalog that rendered it.
//! let catalog = SchemaCatalog::new(vec![TableDef::new(
//!     "users",
//!     vec![
//!         ColumnDef::new("email", TypeKind::Text, false),
//!         ColumnDef::new("id", TypeKind::Int64, false),
//!     ],
//!     vec!["id".to_owned()],
//! )]);
//!
//! let lock = catalog.to_lock();
//!
//! // The snapshot is the versioned TOML v4 document: the first line carries
//! // the format version and the reader accepts the document straight back.
//! assert!(lock.starts_with("version = 4"));
//! assert_eq!(SchemaCatalog::from_lock(&lock).unwrap(), catalog);
//! assert_eq!(catalog.schema_hash().len(), 64);
//! ```

mod lock;
mod schema;
mod types;

pub use schema::SchemaCatalog;
pub use types::{
    CheckDef, ColumnDef, EnumDef, ForeignKeyAction, ForeignKeyDef, IndexDef, TableDef, TriggerDef,
    TypeKind, ViewDef,
};

#[cfg(test)]
mod tests;
