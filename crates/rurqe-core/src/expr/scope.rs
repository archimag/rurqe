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

// A right operand spanning several tables is appended to the left scope as a
// whole, so combining two compound predicates keeps every referenced table.
//
// Every combination of a supported left scope and a right scope of up to four
// tables is spelled out instead of being folded through a recursive bound: a
// recursive definition makes the compiler diverge while it probes a method call
// whose argument types are not inferred yet, which surfaces as a trait-selection
// overflow at the call site rather than as a diagnostic about the query.

macro_rules! impl_empty_left_merge_scope {
    ($($right:ident),+) => {
        impl<$($right: Table),+> MergeScope<($($right,)+)> for () {
            type Output = ($($right,)+);
        }
    };
}

macro_rules! impl_table_left_merge_scope {
    ($($right:ident),+) => {
        impl<L: Table, $($right: Table),+> MergeScope<($($right,)+)> for L {
            type Output = (L, $($right,)+);
        }
    };
}

macro_rules! impl_tuple_left_merge_scope {
    ([$($left:ident),+] [$($right:ident),+]) => {
        impl<$($left: Table,)+ $($right: Table),+> MergeScope<($($right,)+)>
            for ($($left,)+)
        {
            type Output = ($($left,)+ $($right,)+);
        }
    };
}

macro_rules! impl_merged_tuple_scopes {
    ($($right:ident),+) => {
        impl_empty_left_merge_scope!($($right),+);
        impl_table_left_merge_scope!($($right),+);
        impl_tuple_left_merge_scope!([T1] [$($right),+]);
        impl_tuple_left_merge_scope!([T1, T2] [$($right),+]);
        impl_tuple_left_merge_scope!([T1, T2, T3] [$($right),+]);
    };
}

impl_merged_tuple_scopes!(R1);
impl_merged_tuple_scopes!(R1, R2);
impl_merged_tuple_scopes!(R1, R2, R3);
impl_merged_tuple_scopes!(R1, R2, R3, R4);

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
