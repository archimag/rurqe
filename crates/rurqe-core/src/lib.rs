//! Core type-safe SQL AST, schema traits, and dialect abstractions for RURQE.

pub mod schema;

pub use schema::{Column, ColumnBelongsTo, Table, TableInScope};
