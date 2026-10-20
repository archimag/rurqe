//! Combination of the table scopes carried by compared operands.
//!
//! Every operand of a comparison carries the tables its columns belong to: a
//! bound value references no table and keeps the empty scope `()`, a column
//! marker carries its own table, and a compound expression carries every table
//! its operands referenced. Comparing two operands combines their scopes, so
//! `order_user_id.eq(user_id)` is checked against both tables at the call site.

use crate::schema::{CombineScope, Table};

/// The scope produced by combining the operand scopes `Self` and `R`.
#[diagnostic::on_unimplemented(
    message = "the operand scopes `{Self}` and `{R}` cannot be combined",
    label = "unsupported operand scope",
    note = "operands may reference no table, a single table, or a tuple of tables"
)]
pub trait MergeScope<R> {
    /// The scope referencing the tables of both `Self` and `R`.
    type Output;
}

/// The scope produced by combining the operand scopes `L` and `R`.
pub type Merged<L, R> = <L as MergeScope<R>>::Output;

// An operand referencing no table adds nothing to the merged scope.
impl<L> MergeScope<()> for L {
    type Output = L;
}

// The left operand references no table, so the merged scope is its right one.
impl<R: Table> MergeScope<R> for () {
    type Output = <() as CombineScope<R>>::Combined;
}

// Two single-table operands reference both of their tables.
impl<L: Table, R: Table> MergeScope<R> for L {
    type Output = <L as CombineScope<R>>::Combined;
}

macro_rules! impl_tuple_merge_scope {
    ($($member:ident),+) => {
        impl<$($member: Table),+, R: Table> MergeScope<R> for ($($member,)+) {
            type Output = <($($member,)+) as CombineScope<R>>::Combined;
        }
    };
}

impl_tuple_merge_scope!(T1);
impl_tuple_merge_scope!(T1, T2);
impl_tuple_merge_scope!(T1, T2, T3);
impl_tuple_merge_scope!(T1, T2, T3, T4);
impl_tuple_merge_scope!(T1, T2, T3, T4, T5);
impl_tuple_merge_scope!(T1, T2, T3, T4, T5, T6);
impl_tuple_merge_scope!(T1, T2, T3, T4, T5, T6, T7);

// A right operand spanning several tables folds into the left scope one table
// at a time, so combining two compound predicates keeps every referenced table.
impl<L, A: Table, B: Table> MergeScope<(A, B)> for L
where
    L: MergeScope<A>,
    Merged<L, A>: MergeScope<B>,
{
    type Output = Merged<Merged<L, A>, B>;
}

impl<L, A: Table, B: Table, C: Table> MergeScope<(A, B, C)> for L
where
    L: MergeScope<A>,
    Merged<L, A>: MergeScope<B>,
    Merged<Merged<L, A>, B>: MergeScope<C>,
{
    type Output = Merged<Merged<Merged<L, A>, B>, C>;
}

impl<L, A: Table, B: Table, C: Table, D: Table> MergeScope<(A, B, C, D)> for L
where
    L: MergeScope<A>,
    Merged<L, A>: MergeScope<B>,
    Merged<Merged<L, A>, B>: MergeScope<C>,
    Merged<Merged<Merged<L, A>, B>, C>: MergeScope<D>,
{
    type Output = Merged<Merged<Merged<Merged<L, A>, B>, C>, D>;
}

impl<L, A: Table, B: Table, C: Table, D: Table, E: Table> MergeScope<(A, B, C, D, E)> for L
where
    L: MergeScope<A>,
    Merged<L, A>: MergeScope<B>,
    Merged<Merged<L, A>, B>: MergeScope<C>,
    Merged<Merged<Merged<L, A>, B>, C>: MergeScope<D>,
    Merged<Merged<Merged<Merged<L, A>, B>, C>, D>: MergeScope<E>,
{
    type Output = Merged<Merged<Merged<Merged<Merged<L, A>, B>, C>, D>, E>;
}

impl<L, A: Table, B: Table, C: Table, D: Table, E: Table, F: Table>
    MergeScope<(A, B, C, D, E, F)> for L
where
    L: MergeScope<A>,
    Merged<L, A>: MergeScope<B>,
    Merged<Merged<L, A>, B>: MergeScope<C>,
    Merged<Merged<Merged<L, A>, B>, C>: MergeScope<D>,
    Merged<Merged<Merged<Merged<L, A>, B>, C>, D>: MergeScope<E>,
    Merged<Merged<Merged<Merged<Merged<L, A>, B>, C>, D>, E>: MergeScope<F>,
{
    type Output = Merged<Merged<Merged<Merged<Merged<Merged<L, A>, B>, C>, D>, E>, F>;
}

impl<L, A: Table, B: Table, C: Table, D: Table, E: Table, F: Table, G: Table>
    MergeScope<(A, B, C, D, E, F, G)> for L
where
    L: MergeScope<A>,
    Merged<L, A>: MergeScope<B>,
    Merged<Merged<L, A>, B>: MergeScope<C>,
    Merged<Merged<Merged<L, A>, B>, C>: MergeScope<D>,
    Merged<Merged<Merged<Merged<L, A>, B>, C>, D>: MergeScope<E>,
    Merged<Merged<Merged<Merged<Merged<L, A>, B>, C>, D>, E>: MergeScope<F>,
    Merged<Merged<Merged<Merged<Merged<Merged<L, A>, B>, C>, D>, E>, F>: MergeScope<G>,
{
    type Output = Merged<Merged<Merged<Merged<Merged<Merged<Merged<L, A>, B>, C>, D>, E>, F>, G>;
}

impl<L, A: Table, B: Table, C: Table, D: Table, E: Table, F: Table, G: Table, H: Table>
    MergeScope<(A, B, C, D, E, F, G, H)> for L
where
    L: MergeScope<A>,
    Merged<L, A>: MergeScope<B>,
    Merged<Merged<L, A>, B>: MergeScope<C>,
    Merged<Merged<Merged<L, A>, B>, C>: MergeScope<D>,
    Merged<Merged<Merged<Merged<L, A>, B>, C>, D>: MergeScope<E>,
    Merged<Merged<Merged<Merged<Merged<L, A>, B>, C>, D>, E>: MergeScope<F>,
    Merged<Merged<Merged<Merged<Merged<Merged<L, A>, B>, C>, D>, E>, F>: MergeScope<G>,
    Merged<Merged<Merged<Merged<Merged<Merged<Merged<L, A>, B>, C>, D>, E>, F>, G>: MergeScope<H>,
{
    type Output = Merged<
        Merged<Merged<Merged<Merged<Merged<Merged<Merged<L, A>, B>, C>, D>, E>, F>, G>,
        H,
    >;
}

#[cfg(test)]
mod tests {
    use super::*;

    crate::table! {
        User => "users" {
            UserId => "id": i64,
        }
    }

    crate::table! {
        Order => "orders" {
            OrderId => "id": i64,
        }
    }

    fn assert_merged<L: MergeScope<R, Output = Expected>, R, Expected>() {}

    #[test]
    fn empty_operand_scopes_change_nothing() {
        assert_merged::<(), (), ()>();
        assert_merged::<User, (), User>();
        assert_merged::<(User, Order), (), (User, Order)>();
    }

    #[test]
    fn single_table_operands_combine_into_their_pair() {
        assert_merged::<(), User, User>();
        assert_merged::<User, Order, (User, Order)>();
        assert_merged::<(User, Order), User, (User, Order, User)>();
    }

    #[test]
    fn compound_right_operands_fold_into_the_left_scope() {
        assert_merged::<(), (User, Order), (User, Order)>();
        assert_merged::<User, (Order, User), (User, Order, User)>();
        assert_merged::<(User, Order), (User, Order), (User, Order, User, Order)>();
    }
}
