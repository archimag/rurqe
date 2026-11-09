//! Operand compatibility, conversion, and call-site expression traits.
//!
//! The traits here answer the questions an operand raises at the call site of a
//! comparison or an arithmetic operator: [`SqlCompatible`] asserts that the Rust
//! types line up, [`IntoExpr`] converts a bound value into an expression node,
//! [`Expression`] lets a column marker be written directly,
//! [`NumericOperand`] admits the right-hand side of arithmetic, and
//! [`AggregateOperand`] admits the argument of an aggregate function call.
//! [`CombineAggr`] then derives whether the arithmetic still collapses the rows
//! of a group.
//!
//! The traits are split into focused submodules and re-exported here, so callers
//! keep using the flat `ops::...` paths.

mod aggregate;
mod compat;
mod conversion;
mod expression;
mod numeric;
mod subquery;

pub use aggregate::AggregateOperand;
pub use compat::SqlCompatible;
pub use conversion::{IntoExpr, IntoExprList};
pub use expression::{outer, Expression, StringExpression};
pub use numeric::{CombineAggr, CombinedAggr, Numeric, NumericOperand};
pub use subquery::{AnySubquery, InSubquery};
