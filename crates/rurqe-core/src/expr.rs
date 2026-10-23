//! Typed SQL expression AST, bound parameter values, and predicate builders.
//!
//! Expressions are typed by the Rust type of their result: referencing a column
//! yields an expression carrying the column's [`RustType`](crate::schema::Column::RustType),
//! while comparing it with a value yields a boolean predicate. Operand
//! compatibility is checked at the call site through [`SqlCompatible`], so a
//! comparison between unrelated types simply does not compile.
//!
//! Beside the expression nodes, the module hosts the dialect-neutral statement
//! AST ([`SelectAst`]) the query builder renders, so expressions and statements
//! share one vocabulary of nodes.
//!
//! The module is split into focused submodules and re-exported here, so callers
//! keep using the flat `expr::...` paths.

mod array;
mod ast;
mod builder;
mod functions;
mod ops;
mod qualify;
mod render;
mod row;
mod scope;
mod val;

#[cfg(test)]
mod tests;

pub use array::SqlArrayElement;
pub use ast::{
    CaseBranch, CaseBuilder, CteAst, DialectInScope, ExprKind, HasBranches, JoinAst, JoinKind,
    LockClause, LockStrength, LockWait, MergeDialect, MergedDialect, NoBranches, NullsOrder,
    OrderCriterion, OrderDirection, RequiresAll, RequiresDistinctFrom, RequiresGreatest,
    RequiresNullsOrdering, SelectAst, SetOpAst, SetOpKind, Subquery, SubqueryOp, WindowExprKind,
    WindowFrame, WindowFrameBound, WindowFrameKind, DerivedAst,
};
pub use builder::{Aggregate, Expr, NonAggregate};
pub use functions::{
    avg, avg_distinct, coalesce, count, count_all, count_distinct, greatest, is_distinct_from,
    is_not_distinct_from, least, max, min, nullif, sum, sum_distinct, SqlCastTarget,
};
pub use ops::{
    CombineAggr, CombinedAggr, Expression, InSubquery, IntoExpr, IntoExprList, Numeric,
    NumericOperand, SqlCompatible, StringExpression,
};
pub use render::{parse_raw_sql, render_expr, RawSqlError};
pub use row::{RowExpr, RowValues};
pub use scope::{MergeScope, Merged, Unscoped};
pub use val::{AggregateFunc, BinaryOp, SqlVal, ToSqlVal};
#[cfg(feature = "serde_json")]
pub use val::Json;
