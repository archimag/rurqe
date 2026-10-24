//! Type-safe `SELECT` builder and its single-pass SQL renderer.
//!
//! A [`SelectQuery`] is parameterised by the target [`Dialect`], the table in
//! scope, and the Rust type of the produced row. Builder methods only change the
//! phantom parameters, so no SQL exists until [`SelectQuery::build`] performs a
//! deterministic rendering pass that returns the SQL text together with the
//! ordered list of bound parameters.
//!
//! ```text
//! SELECT "id", "name" FROM "users" WHERE ("id" > $1) LIMIT 10
//! ```
//!
//! Joining another table keeps the scope at the type level, so the rendered
//! `FROM` clause always matches the tables the query was built with:
//!
//! ```text
//! SELECT "id", "name" FROM "users" INNER JOIN "orders" ON ("user_id" = $1)
//! ```
//!
//! Rendering and execution are decoupled: [`ExecutableQuery`] exposes both the
//! rendered [`BuiltQuery`] and the row type of every query builder, while
//! [`QueryBackend`] translates the typed [`crate::expr::SqlVal`] parameters into
//! native driver values. [`QueryBackendExt`] adds `fetch_all`, `fetch_one`,
//! `fetch_optional`, and `execute`, decoding every returned row through
//! [`crate::row::FromRow`].

mod cte;
mod execution;
mod group_by;
mod savepoint;
mod scope;
mod select;
mod selection;
mod set_ops;
mod transaction;
mod window;

#[cfg(test)]
mod tests;

pub use self::cte::{cte, cte_named, lateral, Cte, DerivedTable, Lateral};
pub use self::execution::{
    AsyncQueryBackend, AsyncQueryBackendExt, ExecutableQuery, ExecutionError, IsolationLevel,
    QueryBackend, QueryBackendExt, TransactionBackend, TransactionContext,
};
pub use self::group_by::{
    ColumnInGroupBy, EmptyGroupSet, GroupBy, GroupByColumns, GroupByInScope, GroupSet,
    GroupSetColumns, GroupedQuery, GroupedSelectTarget, GroupedSelection, HavingPredicate,
    WindowColsInGroupBy,
};
pub use self::scope::{
    CrossJoined, CrossLateralJoined, FullOuterJoined, JoinKind, Joined, LeftJoined,
    LeftLateralJoined, NonNullableRel, NullableRel, ScopePositions, TableNullabilityInScope,
    TableScope,
};
pub use self::select::{
    BuiltQuery, HasProjection, LockUnavailable, Locked, LockedQuery, NoLock, NoProjection,
    NullsOrder, OrderDirection, ProjectionPresent, Query, SelectQuery, SetOperand,
};
pub use self::selection::{
    ApplyNullability, ColumnInScope, MakeNullable, Nullable, SelectTarget, SelectTargetInScope,
    Selection, SelectionFor, SelectionInScope, TableSelection,
};
pub use self::savepoint::{InvalidSavepointName, SavepointName};
pub use self::set_ops::{SetOpAst, SetOpKind};
pub use self::transaction::{TransactionGuard, TransactionStatus, TransactionalConnection};
pub use self::window::{
    dense_rank, lag, lead, rank, row_number, LagLeadFn, PartitionBy, Window, WindowColumnSet,
    WindowExpr, WindowExprKind, WindowFn, WindowSpec,
};
