//! Position markers and the scope contracts tying columns to tables.

use crate::expr::Unscoped;
use crate::schema::table::Table;

/// Position marker used to disambiguate table membership in composite scopes.
///
/// Lone tables use position 0 ([`At<0>`]), while composite scopes such as `(T1, T2)`
/// assign positions `At<1>`, `At<2>`, etc. to each member.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct At<const N: usize>;

/// A position marker nested one level deeper than `P`.
///
/// Joined query scopes such as [`Joined`](crate::query::Joined) wrap the
/// positions of their inner scope in `Next`, so a table stays reachable exactly
/// once per scope without const-generic arithmetic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Next<P>(pub(crate) core::marker::PhantomData<P>);

/// Statically asserts that the [`Table`] `T` is part of the query scope.
///
/// Expressions may only reference tables that were added to the query, and every
/// table is trivially in scope of itself. Composite scopes represented as tuples
/// contain each of their member tables at distinct positions.
///
/// ```compile_fail
/// use rurqe_core::schema::{Table, TableInScope};
///
/// rurqe_core::table!(
///     User => "users" { UserId => "id": i64, }
/// );
///
/// rurqe_core::table!(
///     Order => "orders" { OrderId => "id": i64, }
/// );
///
/// fn require_in_scope<T: Table, S: TableInScope<T, P>, P>() {}
///
/// // Fails to compile because `Order` is not in scope of `User`:
/// require_in_scope::<Order, User, _>();
/// ```
#[diagnostic::on_unimplemented(
    message = "table `{T}` is not part of this query's scope",
    label = "add `{T}` to the query before referencing its columns",
    note = "only tables introduced into the query scope can be used by its expressions"
)]
pub trait TableInScope<T: Table, P = At<0>> {}

impl<T: Table> TableInScope<T, At<0>> for T {}

macro_rules! impl_tuple_scope {
    ($(($elem:ident, $pos:expr) in ($($member:ident),+);)+) => {
        $(
            impl<$($member: Table),+> TableInScope<$elem, At<$pos>> for ($($member,)+) {}
        )+
    };
}

impl_tuple_scope! {
    (T1, 1) in (T1);

    (T1, 1) in (T1, T2);
    (T2, 2) in (T1, T2);

    (T1, 1) in (T1, T2, T3);
    (T2, 2) in (T1, T2, T3);
    (T3, 3) in (T1, T2, T3);

    (T1, 1) in (T1, T2, T3, T4);
    (T2, 2) in (T1, T2, T3, T4);
    (T3, 3) in (T1, T2, T3, T4);
    (T4, 4) in (T1, T2, T3, T4);

    (T1, 1) in (T1, T2, T3, T4, T5);
    (T2, 2) in (T1, T2, T3, T4, T5);
    (T3, 3) in (T1, T2, T3, T4, T5);
    (T4, 4) in (T1, T2, T3, T4, T5);
    (T5, 5) in (T1, T2, T3, T4, T5);

    (T1, 1) in (T1, T2, T3, T4, T5, T6);
    (T2, 2) in (T1, T2, T3, T4, T5, T6);
    (T3, 3) in (T1, T2, T3, T4, T5, T6);
    (T4, 4) in (T1, T2, T3, T4, T5, T6);
    (T5, 5) in (T1, T2, T3, T4, T5, T6);
    (T6, 6) in (T1, T2, T3, T4, T5, T6);

    (T1, 1) in (T1, T2, T3, T4, T5, T6, T7);
    (T2, 2) in (T1, T2, T3, T4, T5, T6, T7);
    (T3, 3) in (T1, T2, T3, T4, T5, T6, T7);
    (T4, 4) in (T1, T2, T3, T4, T5, T6, T7);
    (T5, 5) in (T1, T2, T3, T4, T5, T6, T7);
    (T6, 6) in (T1, T2, T3, T4, T5, T6, T7);
    (T7, 7) in (T1, T2, T3, T4, T5, T6, T7);

    (T1, 1) in (T1, T2, T3, T4, T5, T6, T7, T8);
    (T2, 2) in (T1, T2, T3, T4, T5, T6, T7, T8);
    (T3, 3) in (T1, T2, T3, T4, T5, T6, T7, T8);
    (T4, 4) in (T1, T2, T3, T4, T5, T6, T7, T8);
    (T5, 5) in (T1, T2, T3, T4, T5, T6, T7, T8);
    (T6, 6) in (T1, T2, T3, T4, T5, T6, T7, T8);
    (T7, 7) in (T1, T2, T3, T4, T5, T6, T7, T8);
    (T8, 8) in (T1, T2, T3, T4, T5, T6, T7, T8);
}

/// Statically asserts that `T` denotes a valid expression scope.
///
/// A scope is the set of relations an expression may reference: a lone table
/// ([`Table`]), a tuple of tables, or a joined query scope such as
/// [`Joined`](crate::query::Joined) and
/// [`LeftJoined`](crate::query::LeftJoined). The marker keeps expression
/// builders honest across joins without pinning them to one concrete scope
/// representation.
#[diagnostic::on_unimplemented(
    message = "`{Self}` is not a valid expression scope",
    label = "expressions may only reference tables, table tuples, or joined scopes",
    note = "build expressions against a table, a tuple of tables, or a `Joined` / `LeftJoined` scope"
)]
pub trait ExprScope {}

impl<T: Table> ExprScope for T {}

macro_rules! impl_tuple_expr_scope {
    ($($member:ident),+) => {
        impl<$($member: Table),+> ExprScope for ($($member,)+) {}
    };
}

impl_tuple_expr_scope!(T1);

impl_tuple_expr_scope!(T1, T2);

impl_tuple_expr_scope!(T1, T2, T3);

impl_tuple_expr_scope!(T1, T2, T3, T4);

impl_tuple_expr_scope!(T1, T2, T3, T4, T5);

impl_tuple_expr_scope!(T1, T2, T3, T4, T5, T6);

impl_tuple_expr_scope!(T1, T2, T3, T4, T5, T6, T7);

impl_tuple_expr_scope!(T1, T2, T3, T4, T5, T6, T7, T8);

/// Statically asserts that every table an expression references is part of the
/// query scope.
///
/// The scope of an expression is the set of tables its columns belong to, which
/// [`Expr`](crate::expr::Expr) carries as its scope parameter. A reference to a
/// single column yields the bare [`Table`], a reference spanning several tables
/// yields a tuple of tables, and an expression referencing no table at all
/// carries `()`. Bounding the scope by `ExprInScope<Tables, _>` therefore
/// rejects a reference to a table that was never added to the query at the call
/// site.
///
/// ```compile_fail
/// use rurqe_core::schema::{ExprInScope, Table, TableInScope};
///
/// rurqe_core::table!(
///     User => "users" { UserId => "id": i64, }
/// );
///
/// rurqe_core::table!(
///     Order => "orders" { OrderId => "id": i64, }
/// );
///
/// fn require_in_scope<S, Tables, P>()
/// where
///     S: ExprInScope<Tables, P>,
/// {
/// }
///
/// // Fails to compile because `Order` is not part of the `users` scope:
/// require_in_scope::<Order, User, _>();
/// ```
#[diagnostic::on_unimplemented(
    message = "expression references table `{Self}`, which is not part of this query's scope",
    label = "table `{Self}` is not in scope for this query",
    note = "add `{Self}` to the query via `from`, `inner_join`, or `left_join` before referencing its columns"
)]
pub trait ExprInScope<Tables, Positions = At<0>> {}

impl<Tables, Positions> ExprInScope<Tables, Positions> for () {}

// An expression whose scope was dropped declares that its tables are checked by
// the statement enclosing it, so it is accepted in every scope — while staying
// distinguishable from an expression that references no table at all.
impl<Tables, Positions> ExprInScope<Tables, Positions> for Unscoped {}

impl<T: Table, Tables, P> ExprInScope<Tables, P> for T where Tables: TableInScope<T, P> {}

macro_rules! impl_tuple_expr_in_scope {
    ($($member:ident : $pos:ident),+) => {
        impl<$($member: Table),+, Tables, $($pos),+> ExprInScope<Tables, ($($pos,)+)>
            for ($($member,)+)
        where
            $(Tables: TableInScope<$member, $pos>),+
        {
        }
    };
}

impl_tuple_expr_in_scope!(T1: P1);

impl_tuple_expr_in_scope!(T1: P1, T2: P2);

impl_tuple_expr_in_scope!(T1: P1, T2: P2, T3: P3);

impl_tuple_expr_in_scope!(T1: P1, T2: P2, T3: P3, T4: P4);

impl_tuple_expr_in_scope!(T1: P1, T2: P2, T3: P3, T4: P4, T5: P5);

impl_tuple_expr_in_scope!(T1: P1, T2: P2, T3: P3, T4: P4, T5: P5, T6: P6);

impl_tuple_expr_in_scope!(T1: P1, T2: P2, T3: P3, T4: P4, T5: P5, T6: P6, T7: P7);

impl_tuple_expr_in_scope!(T1: P1, T2: P2, T3: P3, T4: P4, T5: P5, T6: P6, T7: P7, T8: P8);

/// Combines two expression scopes into the scope referencing both of them.
///
/// Joins extend the scope of a query by the tables they introduce, and an
/// expression built from columns of several tables carries the union of their
/// scopes. The trait names that union at the type level: `()` contributes
/// nothing, a single table contributes itself, and any further table extends a
/// tuple of references from the left.
///
/// ```
/// use rurqe_core::schema::CombineScope;
///
/// rurqe_core::table!(
///     User => "users" { UserId => "id": i64, }
/// );
///
/// rurqe_core::table!(
///     Order => "orders" { OrderId => "id": i64, }
/// );
///
/// fn assert_combined<A: CombineScope<B, Combined = C>, B, C>() {}
///
/// assert_combined::<User, Order, (User, Order)>();
/// assert_combined::<(User, Order), User, (User, Order, User)>();
/// ```
pub trait CombineScope<Other> {
    /// The scope referencing both `Self` and `Other`.
    type Combined;
}

impl CombineScope<()> for () {
    type Combined = ();
}

impl<T: Table> CombineScope<()> for T {
    type Combined = T;
}

impl<T: Table> CombineScope<T> for () {
    type Combined = T;
}

impl<T1: Table, T2: Table> CombineScope<T2> for T1 {
    type Combined = (T1, T2);
}

macro_rules! impl_tuple_combine_scope {
    ($($member:ident),+) => {
        impl<$($member: Table),+, Added: Table> CombineScope<Added> for ($($member,)+) {
            type Combined = ($($member,)+ Added);
        }
    };
}

impl_tuple_combine_scope!(T1);

impl_tuple_combine_scope!(T1, T2);

impl_tuple_combine_scope!(T1, T2, T3);

impl_tuple_combine_scope!(T1, T2, T3, T4);

impl_tuple_combine_scope!(T1, T2, T3, T4, T5);

impl_tuple_combine_scope!(T1, T2, T3, T4, T5, T6);

impl_tuple_combine_scope!(T1, T2, T3, T4, T5, T6, T7);

/// Trait for extending a relational scope with an additional table.
pub trait ExtendScope<S: Table> {
    /// The resulting composite scope after adding `S`.
    type Output;
}

impl<T: Table, S: Table> ExtendScope<S> for T {
    type Output = (T, S);
}

impl<T1: Table, S: Table> ExtendScope<S> for (T1,) {
    type Output = (T1, S);
}

impl<T1: Table, T2: Table, S: Table> ExtendScope<S> for (T1, T2) {
    type Output = (T1, T2, S);
}

impl<T1: Table, T2: Table, T3: Table, S: Table> ExtendScope<S> for (T1, T2, T3) {
    type Output = (T1, T2, T3, S);
}

impl<T1: Table, T2: Table, T3: Table, T4: Table, S: Table> ExtendScope<S> for (T1, T2, T3, T4) {
    type Output = (T1, T2, T3, T4, S);
}

impl<T1: Table, T2: Table, T3: Table, T4: Table, T5: Table, S: Table> ExtendScope<S>
    for (T1, T2, T3, T4, T5)
{
    type Output = (T1, T2, T3, T4, T5, S);
}

impl<T1: Table, T2: Table, T3: Table, T4: Table, T5: Table, T6: Table, S: Table> ExtendScope<S>
    for (T1, T2, T3, T4, T5, T6)
{
    type Output = (T1, T2, T3, T4, T5, T6, S);
}

impl<T1: Table, T2: Table, T3: Table, T4: Table, T5: Table, T6: Table, T7: Table, S: Table>
    ExtendScope<S> for (T1, T2, T3, T4, T5, T6, T7)
{
    type Output = (T1, T2, T3, T4, T5, T6, T7, S);
}
