//! Relation scopes of the values recorded by column assignments.
//!
//! A bound value references no table, while an expression assignment reads the
//! tables of its operands. [`AssignmentScope`] names the union of those tables,
//! so a statement checks the relations its assigned values read exactly like the
//! relations of its filter.

use crate::expr::{MergeScope, Merged};
use crate::schema::Column;

use super::Assignment;

/// The relation scope of the values recorded by a column assignment.
///
/// A bound value reports the empty scope `()`, an expression assignment reports
/// the tables its operands read, and a tuple reports the union of the scopes of
/// its elements. `UPDATE` therefore validates the tables its assigned expressions
/// read against the relations the statement addresses, exactly like the tables of
/// its filter:
///
/// ```text
/// UPDATE "users" SET "id" = "accounts"."id" FROM "accounts"
/// ```
///
/// An assignment reading a table the statement does not address is rejected at
/// compile time:
///
/// ```compile_fail
/// use rurqe_core::dialect::Dialect;
/// use rurqe_core::dml::{ColumnAssign, UpdateQuery};
/// use rurqe_core::expr::Expr;
///
/// struct MockDialect;
///
/// impl Dialect for MockDialect {
///     fn push_quoted_identifier(out: &mut String, ident: &str) {
///         out.push_str(ident);
///     }
///
///     fn push_placeholder(out: &mut String, index: usize) {
///         let _ = index;
///     }
/// }
///
/// rurqe_core::table! {
///     User => "users" {
///         UserId => "id": i64,
///     }
/// }
///
/// rurqe_core::table! {
///     Account => "accounts" {
///         AccountId => "id": i64,
///     }
/// }
///
/// // Fails to compile: the assigned expression reads `Account`, which the
/// // statement does not address.
/// let _ = UpdateQuery::<MockDialect, User>::new()
///     .set(UserId::set_expr(Expr::column::<AccountId>()))
///     .allow_all();
/// ```
#[diagnostic::on_unimplemented(
    message = "`{Self}` records no column value",
    label = "only typed column assignments carry a relation scope",
    note = "record values through `Column::set`, `Column::default_value`, or `Column::set_expr`"
)]
pub trait AssignmentScope {
    /// The tables the recorded values read.
    type Scope;
}

impl<C, V, Scope> AssignmentScope for Assignment<C, V, Scope>
where
    C: Column,
{
    type Scope = Scope;
}

/// The scope of a two-element assignment tuple.
type TupleScope2<A, B> = Merged<<A as AssignmentScope>::Scope, <B as AssignmentScope>::Scope>;

/// The scope of a three-element assignment tuple.
type TupleScope3<A, B, C> = Merged<TupleScope2<A, B>, <C as AssignmentScope>::Scope>;

/// The scope of a four-element assignment tuple.
type TupleScope4<A, B, C, D> = Merged<TupleScope3<A, B, C>, <D as AssignmentScope>::Scope>;

/// The scope of a five-element assignment tuple.
type TupleScope5<A, B, C, D, E> = Merged<TupleScope4<A, B, C, D>, <E as AssignmentScope>::Scope>;

/// The scope of a six-element assignment tuple.
type TupleScope6<A, B, C, D, E, F> =
    Merged<TupleScope5<A, B, C, D, E>, <F as AssignmentScope>::Scope>;

/// The scope of a seven-element assignment tuple.
type TupleScope7<A, B, C, D, E, F, G> =
    Merged<TupleScope6<A, B, C, D, E, F>, <G as AssignmentScope>::Scope>;

/// The scope of an eight-element assignment tuple.
type TupleScope8<A, B, C, D, E, F, G, H> =
    Merged<TupleScope7<A, B, C, D, E, F, G>, <H as AssignmentScope>::Scope>;

impl<A: AssignmentScope> AssignmentScope for (A,) {
    type Scope = A::Scope;
}

impl<A, B> AssignmentScope for (A, B)
where
    A: AssignmentScope,
    B: AssignmentScope,
    A::Scope: MergeScope<B::Scope>,
{
    type Scope = TupleScope2<A, B>;
}

impl<A, B, C> AssignmentScope for (A, B, C)
where
    A: AssignmentScope,
    B: AssignmentScope,
    C: AssignmentScope,
    A::Scope: MergeScope<B::Scope>,
    TupleScope2<A, B>: MergeScope<C::Scope>,
{
    type Scope = TupleScope3<A, B, C>;
}

impl<A, B, C, D> AssignmentScope for (A, B, C, D)
where
    A: AssignmentScope,
    B: AssignmentScope,
    C: AssignmentScope,
    D: AssignmentScope,
    A::Scope: MergeScope<B::Scope>,
    TupleScope2<A, B>: MergeScope<C::Scope>,
    TupleScope3<A, B, C>: MergeScope<D::Scope>,
{
    type Scope = TupleScope4<A, B, C, D>;
}

impl<A, B, C, D, E> AssignmentScope for (A, B, C, D, E)
where
    A: AssignmentScope,
    B: AssignmentScope,
    C: AssignmentScope,
    D: AssignmentScope,
    E: AssignmentScope,
    A::Scope: MergeScope<B::Scope>,
    TupleScope2<A, B>: MergeScope<C::Scope>,
    TupleScope3<A, B, C>: MergeScope<D::Scope>,
    TupleScope4<A, B, C, D>: MergeScope<E::Scope>,
{
    type Scope = TupleScope5<A, B, C, D, E>;
}

impl<A, B, C, D, E, F> AssignmentScope for (A, B, C, D, E, F)
where
    A: AssignmentScope,
    B: AssignmentScope,
    C: AssignmentScope,
    D: AssignmentScope,
    E: AssignmentScope,
    F: AssignmentScope,
    A::Scope: MergeScope<B::Scope>,
    TupleScope2<A, B>: MergeScope<C::Scope>,
    TupleScope3<A, B, C>: MergeScope<D::Scope>,
    TupleScope4<A, B, C, D>: MergeScope<E::Scope>,
    TupleScope5<A, B, C, D, E>: MergeScope<F::Scope>,
{
    type Scope = TupleScope6<A, B, C, D, E, F>;
}

impl<A, B, C, D, E, F, G> AssignmentScope for (A, B, C, D, E, F, G)
where
    A: AssignmentScope,
    B: AssignmentScope,
    C: AssignmentScope,
    D: AssignmentScope,
    E: AssignmentScope,
    F: AssignmentScope,
    G: AssignmentScope,
    A::Scope: MergeScope<B::Scope>,
    TupleScope2<A, B>: MergeScope<C::Scope>,
    TupleScope3<A, B, C>: MergeScope<D::Scope>,
    TupleScope4<A, B, C, D>: MergeScope<E::Scope>,
    TupleScope5<A, B, C, D, E>: MergeScope<F::Scope>,
    TupleScope6<A, B, C, D, E, F>: MergeScope<G::Scope>,
{
    type Scope = TupleScope7<A, B, C, D, E, F, G>;
}

impl<A, B, C, D, E, F, G, H> AssignmentScope for (A, B, C, D, E, F, G, H)
where
    A: AssignmentScope,
    B: AssignmentScope,
    C: AssignmentScope,
    D: AssignmentScope,
    E: AssignmentScope,
    F: AssignmentScope,
    G: AssignmentScope,
    H: AssignmentScope,
    A::Scope: MergeScope<B::Scope>,
    TupleScope2<A, B>: MergeScope<C::Scope>,
    TupleScope3<A, B, C>: MergeScope<D::Scope>,
    TupleScope4<A, B, C, D>: MergeScope<E::Scope>,
    TupleScope5<A, B, C, D, E>: MergeScope<F::Scope>,
    TupleScope6<A, B, C, D, E, F>: MergeScope<G::Scope>,
    TupleScope7<A, B, C, D, E, F, G>: MergeScope<H::Scope>,
{
    type Scope = TupleScope8<A, B, C, D, E, F, G, H>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::expr::Expr;

    crate::table! {
        User => "users" {
            UserId => "id": i64,
            UserName => "name": String,
        }
    }

    crate::table! {
        Account => "accounts" {
            AccountId => "id": i64,
        }
    }

    fn assert_scope<S: AssignmentScope<Scope = Expected>, Expected>() {}

    #[test]
    fn bound_values_read_no_table() {
        assert_scope::<Assignment<UserId, i64>, ()>();
        assert_scope::<Assignment<UserName, &'static str>, ()>();
    }

    #[test]
    fn expression_assignments_carry_the_tables_their_operands_read() {
        assert_scope::<Assignment<UserId, Expr<i64, Account>, Account>, Account>();
    }

    #[test]
    fn tuples_merge_the_scopes_of_their_elements() {
        assert_scope::<
            (
                Assignment<UserId, i64>,
                Assignment<UserName, Expr<String, User>, User>,
            ),
            User,
        >();
        assert_scope::<
            (
                Assignment<UserId, Expr<i64, Account>, Account>,
                Assignment<UserName, Expr<String, User>, User>,
            ),
            (Account, User),
        >();
    }
}
