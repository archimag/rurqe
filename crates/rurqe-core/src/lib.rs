//! Core type-safe SQL AST, schema traits, and dialect abstractions for RURQE.

pub mod dialect;
pub mod dml;
pub mod expr;
pub mod query;
pub mod row;
pub mod schema;

pub use dialect::{Dialect, SupportsDistinctOn, SupportsOnConflict, SupportsReturning};
pub use dml::{AssignColumn, AssignValue, Assignment, AssignmentSet, ColumnAssign};
pub use expr::{AggregateFunc, BinaryOp, Expr, ExprKind, IntoExpr, SqlCompatible, SqlVal};
pub use query::{
    cte, cte_named, dense_rank, lag, lead, rank, row_number, BuiltQuery, Cte, ExecutableQuery,
    ExecutionError, GroupBy, GroupByInScope, IsolationLevel, LagLeadFn, OrderDirection,
    PartitionBy, QueryBackend, QueryBackendExt, SelectQuery, SelectTarget, SelectTargetInScope,
    Selection, SelectionInScope, TransactionBackend, TransactionContext, Window, WindowExpr,
    WindowExprKind, WindowFn, WindowSpec,
};
pub use row::{FromRow, FromSqlValue};
pub use schema::{At, Column, ColumnBelongsTo, Table, TableInScope};
