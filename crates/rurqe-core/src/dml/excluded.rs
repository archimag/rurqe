//! Typed references to the `EXCLUDED` pseudo-relation of an upsert.
//!
//! While the `ON CONFLICT DO UPDATE` branch of an `INSERT` runs, the row the
//! statement tried to insert stays reachable as the `EXCLUDED` relation, so the
//! update copies values from it instead of repeating them as parameters:
//!
//! ```text
//! INSERT INTO "users" ("id", "name") VALUES ($1, $2)
//!     ON CONFLICT ("id") DO UPDATE SET "name" = EXCLUDED."name"
//! ```
//!
//! [`excluded`] builds the typed reference from a column marker and
//! [`ColumnAssign::set_expr`](crate::dml::ColumnAssign::set_expr) records the
//! resulting expression as the value of an assignment, so the proposed row is
//! only reachable through a column of the matching Rust type.

use core::marker::PhantomData;

use crate::expr::{Expr, ExprKind};
use crate::schema::Column;

/// Marker naming the table whose proposed row the `EXCLUDED` relation exposes.
///
/// The marker keeps the referenced table in the type of the built expression, so
/// a value read from the proposed row stays distinguishable from one read from
/// the table the statement inserts into.
pub struct Excluded<T>(PhantomData<fn() -> T>);

/// Builds the expression reading the column `C` of the `EXCLUDED` relation.
///
/// The expression carries the Rust type of `C`, so it only assigns to a column of
/// the same type, and it renders as `EXCLUDED."column"`:
///
/// ```
/// use rurqe_core::dml::excluded;
/// use rurqe_core::expr::ExprKind;
///
/// rurqe_core::table! {
///     User => "users" {
///         UserName => "name": String,
///     }
/// }
///
/// let value = excluded(UserName);
///
/// assert_eq!(value.kind(), &ExprKind::Excluded { column: "name" });
/// ```
pub fn excluded<C: Column>(_column: C) -> Expr<C::RustType, Excluded<C::Table>> {
    Expr::from_kind(ExprKind::Excluded { column: C::NAME })
}
