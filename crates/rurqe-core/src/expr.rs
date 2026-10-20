//! Typed SQL expression AST, bound parameter values, and predicate builders.
//!
//! Expressions are typed by the Rust type of their result: referencing a column
//! yields an expression carrying the column's [`RustType`](crate::schema::Column::RustType),
//! while comparing it with a value yields a boolean predicate. Operand
//! compatibility is checked at the call site through [`SqlCompatible`], so a
//! comparison between unrelated types simply does not compile.
//!
//! The module is split into focused submodules and re-exported here, so callers
//! keep using the flat `expr::...` paths.

mod ast;
mod builder;
mod functions;
mod ops;
mod render;
mod scope;
mod val;

#[cfg(test)]
mod tests;

pub use ast::{CaseBranch, CaseBuilder, ExprKind, HasBranches, NoBranches, Subquery, SubqueryOp};
pub use builder::{Aggregate, Expr, NonAggregate};
pub use functions::{avg, coalesce, count, count_all, max, min, sum};
pub use ops::{Expression, IntoExpr, SqlCompatible, StringExpression};
pub use render::{parse_raw_sql, render_expr, RawSqlError};
pub use scope::{MergeScope, Merged};
pub use val::{AggregateFunc, BinaryOp, SqlVal, ToSqlVal};
#[cfg(feature = "serde_json")]
pub use val::Json;
