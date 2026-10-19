//! Core type-safe SQL AST, schema traits, and dialect abstractions for RURQE.

pub mod dialect;
pub mod dml;
pub mod expr;
pub mod query;
pub mod row;
pub mod schema;

pub use dialect::{Dialect, SupportsDistinctOn, SupportsOnConflict, SupportsReturning};
pub use dml::{AssignColumn, AssignValue, Assignment, AssignmentSet, ColumnAssign};
pub use expr::{AggregateFunc, BinaryOp, Expr, ExprKind, SqlCompatible, SqlVal};
pub use query::{
    dense_rank, lag, lead, rank, row_number, BuiltQuery, ExecutableQuery, ExecutionError, GroupBy,
    IsolationLevel, LagLeadFn, OrderDirection, PartitionBy, QueryBackend, QueryBackendExt,
    SelectQuery, SelectTarget, Selection, TransactionBackend, TransactionContext, Window,
    WindowExpr, WindowExprKind, WindowFn, WindowSpec,
};
pub use row::{FromRow, FromSqlValue};
pub use schema::{At, Column, ColumnBelongsTo, Table, TableInScope};
