//! The operand contract of an aggregate function call.
//!
//! An aggregate reads the rows of a group and collapses them into a single
//! value, so the argument it consumes is a per-row expression: a column marker,
//! an expression built from columns and values, or a fragment spelling a call
//! the typed API does not model. [`AggregateOperand`] states that contract, and
//! it mirrors the shape of `NumericOperand`: the operand states the tables it
//! references and the dialect requirement it carries, so a call keeps the scope
//! of its argument and is accepted by a statement that introduces those tables
//! and by no other.
//!
//! The contract admits per-row expressions alone, which is what keeps a nested
//! aggregate call unwritable. `SUM(SUM(x))` is refused by every engine, because
//! the inner call leaves no row for the outer one to read; one row per group
//! aggregated again is the window form `SUM(SUM(x)) OVER (...)`, which
//! [`Expr::over`](crate::expr::Expr::over) composes over the window builder
//! instead of an aggregate call.

use crate::expr::ast::ExprKind;
use crate::expr::builder::{Expr, NonAggregate};
use crate::schema::Column;

/// A value an aggregate function call aggregates.
///
/// An aggregate collapses the rows of a group, so the value it consumes reads
/// one row at a time: a column marker, or an expression built from columns and
/// values. Beside the tables the operand references, it states the dialect
/// requirement it carries, and the call inherits both — an aggregate over a
/// column of a relation the query never introduces is rejected at the call site
/// exactly like the column reference alone would be.
///
/// ```
/// use rurqe_core::expr::{sum, ExprKind};
///
/// rurqe_core::table!(
///     /// The `orders` table.
///     Order => "orders" {
///         OrderTotal => "total": f64,
///     }
/// );
///
/// // A column marker is an operand of its own.
/// assert!(matches!(sum(OrderTotal).kind(), ExprKind::Aggregate { .. }));
/// ```
///
/// The contract stops at a per-row expression, so an aggregate call is not an
/// operand of another call: the type-level counterpart of the refusal every
/// engine states for a nested aggregate.
///
/// ```compile_fail
/// use rurqe_core::expr::sum;
///
/// rurqe_core::table!(
///     /// The `orders` table.
///     Order => "orders" {
///         OrderTotal => "total": f64,
///     }
/// );
///
/// // Fails to compile: `SUM("total")` is an aggregate call and not an operand
/// // of another one, so the nested call never reaches the engine.
/// let _ = sum(sum(OrderTotal));
/// ```
#[diagnostic::on_unimplemented(
    message = "`{Self}` is not an operand of an aggregate function",
    label = "incompatible aggregate operand",
    note = "an aggregate reads a column marker or a per-row expression built from columns and values; an aggregate call is not an operand of another aggregate call"
)]
pub trait AggregateOperand<T>: Sized {
    /// The scope of the tables the operand references.
    type Scope;

    /// The dialect marker of the operand.
    type Dialect;

    /// Converts `self` into the expression node the call aggregates.
    fn into_aggregate_operand(self) -> ExprKind;
}

// A column marker computes one value per row of its table, so the call keeps the
// table as its scope and a statement that lacks it rejects the aggregate at the
// call site. A column is dialect-neutral: the dialect quoting an identifier is
// the one the enclosing statement renders with.
impl<T, C> AggregateOperand<T> for C
where
    C: Column<RustType = T>,
{
    type Scope = C::Table;
    type Dialect = ();

    fn into_aggregate_operand(self) -> ExprKind {
        C::expr_kind()
    }
}

// A built expression keeps the tables it reads and the dialect requirement it
// carries, so `SUM("total" + $1)` stays in the scope of `total` and subject to
// the dialect its fragments require. The marker is restricted to `NonAggregate`
// on purpose: an expression that already collapses its group reads rows that no
// longer exist, which is exactly what SQL refuses.
impl<T, Scope, D> AggregateOperand<T> for Expr<T, Scope, NonAggregate, D> {
    type Scope = Scope;
    type Dialect = D;

    fn into_aggregate_operand(self) -> ExprKind {
        self.into_kind()
    }
}
