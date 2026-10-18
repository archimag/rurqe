//! Core type-safe SQL AST, schema traits, and dialect abstractions for RURQE.

pub mod dialect;
pub mod expr;
pub mod query;
pub mod row;
pub mod schema;

pub use dialect::{Dialect, SupportsDistinctOn, SupportsOnConflict, SupportsReturning};
pub use expr::{BinaryOp, Expr, ExprKind, SqlCompatible, SqlVal};
pub use query::{BuiltQuery, OrderDirection, SelectQuery, Selection};
pub use row::{FromRow, FromSqlValue};
pub use schema::{Column, ColumnBelongsTo, Table, TableInScope};
