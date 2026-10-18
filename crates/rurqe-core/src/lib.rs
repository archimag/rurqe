//! Core type-safe SQL AST, schema traits, and dialect abstractions for RURQE.

pub mod dialect;
pub mod expr;
pub mod query;
pub mod schema;

pub use dialect::{Dialect, SupportsDistinctOn, SupportsOnConflict, SupportsReturning};
pub use expr::{BinaryOp, Expr, ExprKind, SqlCompatible, SqlVal};
pub use query::{BuiltQuery, SelectQuery, Selection};
pub use schema::{Column, ColumnBelongsTo, Table, TableInScope};
