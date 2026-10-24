//! Core type-safe SQL AST, schema traits, and dialect abstractions for RURQE.

pub mod dialect;
pub mod dml;
pub mod expr;
pub mod nested;
pub mod query;
pub mod row;
pub mod schema;

pub use dialect::{
    Dialect, SupportsDeleteUsing, SupportsDistinctFrom, SupportsDistinctOn, SupportsGreatest,
    SupportsJsonAggregation, SupportsNullsOrdering, SupportsOnConflict, SupportsReturning,
    SupportsRowLocking, SupportsUpdateFrom, SupportsWindowFunctions,
};
pub use dml::{
    excluded, AssignColumn, AssignValue, Assignment, AssignmentScope, AssignmentSet, ColumnAssign,
    DeleteQuery, Excluded, ExecutableDelete, ExecutableUpdate, InsertQuery, Unfiltered,
    UpdateQuery,
};
pub use expr::{
    greatest, is_distinct_from, is_not_distinct_from, least, nullif, Aggregate, AggregateFunc,
    BinaryOp, Expr, ExprKind, IntoExpr, NonAggregate, NullsOrder, SqlArrayElement, SqlCastTarget,
    SqlCompatible, SqlVal, ToSqlVal,
};
pub use nested::{nest_many, nest_one, MultisetRow, NestMany, NestOne};
pub use query::{
    cte, cte_named, dense_rank, lag, lead, rank, row_number, BuiltQuery, Cte, DerivedTable,
    ExecutableQuery, ExecutionError, GroupBy, GroupByInScope, IsolationLevel, LagLeadFn,
    LockedQuery, OrderDirection, PartitionBy, Query, QueryBackend, QueryBackendExt, SelectQuery,
    SelectTarget, SelectTargetInScope, Selection, SelectionInScope, TransactionBackend,
    TransactionContext, Window, WindowExpr, WindowExprKind, WindowFn, WindowSpec,
};
pub use row::{FromRow, FromSqlValue, RowDecodeError};
pub use schema::{At, Column, ColumnBelongsTo, Table, TableInScope};
