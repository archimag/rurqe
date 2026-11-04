//! The grammar of a window operand.
//!
//! An `OVER (...)` clause attaches to a function application and to nothing
//! else, so the operand of a window expression has to render as one. The
//! function here names the shape of an operand that cannot carry the clause,
//! which is the form the pre-flight pass reports before a statement is
//! rendered.

use crate::expr::ExprKind;

/// Names the operand shape of a window expression that cannot carry `OVER (...)`.
///
/// An `OVER` clause follows a function application, so the operand of a window has
/// to render as one: an aggregate call — `SUM("total") OVER (...)` — a scalar
/// function call — `FIRST_VALUE("user_id") OVER (...)` — or a raw fragment trusted
/// to spell such a call. Every other shape names a value rather than a call, so
/// appending the clause to it renders a statement the engine rejects with a syntax
/// error: PostgreSQL answers `"total" OVER (...)` with `syntax error at or near
/// "OVER"`.
///
/// The returned label describes the construct in SQL terms, so the diagnostic names
/// what the caller wrote instead of a node of the expression tree. `None` means the
/// operand is a function application and the clause attaches to it.
pub(super) fn window_operand_shape(operand: &ExprKind) -> Option<&'static str> {
    match operand {
        ExprKind::Aggregate { .. } | ExprKind::Function { .. } | ExprKind::Raw { .. } => None,
        ExprKind::Column { .. } | ExprKind::QualifiedColumn { .. } => Some("a column reference"),
        ExprKind::Excluded { .. } => Some("an `EXCLUDED` column reference"),
        ExprKind::Row { .. } => Some("a row value"),
        ExprKind::Value(_) => Some("a bound value"),
        ExprKind::Binary { .. }
        | ExprKind::DialectOp { .. }
        | ExprKind::IsNull { .. }
        | ExprKind::Like { .. }
        | ExprKind::Between { .. }
        | ExprKind::InList { .. }
        | ExprKind::DistinctFrom { .. } => Some("an operator expression"),
        ExprKind::Subquery { .. } => Some("a subquery"),
        ExprKind::Cast { .. } => Some("a `CAST` expression"),
        ExprKind::Case { .. } => Some("a `CASE` expression"),
        ExprKind::Window { .. } => Some("a window function"),
        ExprKind::Multiset { .. } => Some("a nested collection"),
        ExprKind::Aliased { .. } => Some("an aliased expression"),
    }
}
