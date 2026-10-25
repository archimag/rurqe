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

use crate::expr::{Expr, ExprKind, MergeScope, Unscoped};
use crate::schema::{Column, Table};

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
    // The scope of the proposed row is derived from the column contract, so the
    // expression is assembled with the crate-internal constructor instead of the
    // escape hatch, which builds scope-free constants only.
    Expr::new(ExprKind::Excluded { column: C::NAME })
}

// The proposed row is not a relation of a query scope: it only exists inside the
// update branch of an upsert, so it merges into the scope of a predicate instead
// of extending the scope of a statement. Comparing a stored column with the value
// proposed for it therefore stays typed — the resulting predicate reads both the
// table and its proposed row — and renders as a single condition:
//
// ```text
// DO UPDATE SET "name" = EXCLUDED."name" WHERE ("name" <> EXCLUDED."name")
// ```

impl<T: Table> MergeScope<Excluded<T>> for Excluded<T> {
    type Output = Excluded<T>;
}

impl<L: Table, T: Table> MergeScope<Excluded<T>> for L {
    type Output = (L, Excluded<T>);
}

impl<T: Table, R: Table> MergeScope<R> for Excluded<T> {
    type Output = (Excluded<T>, R);
}

// The right operand of these two impls is the proposed row itself, which stays
// unambiguously distinct from the empty scope and from the dropped-scope marker.
impl<T: Table> MergeScope<Excluded<T>> for () {
    type Output = Excluded<T>;
}

impl<T: Table> MergeScope<Excluded<T>> for Unscoped {
    type Output = Excluded<T>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::expr::Merged;

    crate::table! {
        User => "users" {
            UserName => "name": String,
        }
    }

    fn assert_merged<L: MergeScope<R, Output = Expected>, R, Expected>() {}

    #[test]
    fn the_proposed_row_merges_into_the_scope_of_a_predicate() {
        assert_merged::<Excluded<User>, Excluded<User>, Excluded<User>>();
        assert_merged::<User, Excluded<User>, (User, Excluded<User>)>();
        assert_merged::<Excluded<User>, User, (Excluded<User>, User)>();
        assert_merged::<(), Excluded<User>, Excluded<User>>();
        assert_merged::<Unscoped, Excluded<User>, Excluded<User>>();

        // A constant or unscoped operand on the right contributes no table, so
        // the proposed row survives unchanged.
        assert_merged::<Excluded<User>, (), Excluded<User>>();
        assert_merged::<Excluded<User>, Unscoped, Excluded<User>>();
    }

    #[test]
    fn a_proposed_value_carries_the_type_of_its_column() {
        let proposed: Expr<String, Excluded<User>> = excluded(UserName);

        assert_eq!(proposed.kind(), &ExprKind::Excluded { column: "name" });
    }

    #[test]
    fn a_predicate_comparing_the_stored_and_proposed_rows_keeps_both_scopes() {
        let _: Expr<bool, Merged<User, Excluded<User>>> =
            Expr::column::<UserName>().ne(excluded(UserName));
        let _: Expr<bool, Merged<Excluded<User>, User>> =
            excluded(UserName).ne(Expr::column::<UserName>());
    }
}
