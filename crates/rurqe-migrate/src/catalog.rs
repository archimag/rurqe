//! Engine-neutral schema catalog and its canonical `schema.lock` snapshot.
//!
//! The catalog is the single source of truth shared by introspection, code
//! generation, and drift verification: it records enum types, tables, columns,
//! primary keys, foreign keys, and secondary or unique indexes in an
//! engine-independent form and renders them into a deterministic textual
//! snapshot.
//!
//! Determinism is achieved by sorting enum types, tables, columns, foreign keys,
//! and indexes by name, so two catalogs built in a different declaration order
//! still produce the very same snapshot — and therefore the very same
//! [`SchemaCatalog::schema_hash`].
//!
//! ```
//! use rurqe_migrate::{ColumnDef, SchemaCatalog, TableDef, TypeKind};
//!
//! let catalog = SchemaCatalog::new(vec![TableDef::new(
//!     "users",
//!     vec![
//!         ColumnDef::new("id", TypeKind::Int64, false),
//!         ColumnDef::new("email", TypeKind::Text, false),
//!     ],
//!     vec!["id".to_owned()],
//! )]);
//!
//! assert_eq!(
//!     catalog.to_lock(),
//!     concat!(
//!         "schema lock v1\n",
//!         "\n",
//!         "table \"users\" {\n",
//!         "  column \"email\": text not null\n",
//!         "  column \"id\": int64 not null\n",
//!         "  primary key (\"id\")\n",
//!         "}\n",
//!     )
//! );
//!
//! assert_eq!(catalog.schema_hash().len(), 64);
//! ```

mod lock;
mod schema;
mod types;

pub use schema::SchemaCatalog;
pub use types::{ColumnDef, EnumDef, ForeignKeyDef, IndexDef, TableDef, TypeKind};

#[cfg(test)]
mod tests;
