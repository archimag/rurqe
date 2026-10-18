//! Core type-safe SQL AST, schema traits, and dialect abstractions for RURQE.

pub mod expr;
pub mod schema;

pub use expr::{BinaryOp, Expr, ExprKind, SqlCompatible, SqlVal};
pub use schema::{Column, ColumnBelongsTo, Table, TableInScope};
