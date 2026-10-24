//! Table aliases and the markers referencing a table through an alias.

use core::marker::PhantomData;

use crate::schema::table::{Column, Table, TableRef};

/// A compile-time name under which an [`Aliased`] table is referenced.
///
/// Aliases are declared with the [`table_alias!`](crate::table_alias) macro:
///
/// ```
/// rurqe_core::table_alias! {
///     /// The `u` alias.
///     UserAlias => "u"
/// }
///
/// use rurqe_core::schema::TableAlias;
///
/// assert_eq!(UserAlias::NAME, "u");
/// ```
pub trait TableAlias: Sized {
    /// The SQL identifier of the alias, known at compile time.
    const NAME: &'static str;

    /// Renders `table` under this alias, as used in `FROM` and `JOIN` clauses.
    fn table_ref(table: &'static str) -> TableRef {
        TableRef::aliased(table, Self::NAME)
    }

    /// Builds the expression node referencing `column` of a table through this alias.
    fn column_expr(column: &'static str) -> crate::expr::ExprKind {
        crate::expr::ExprKind::QualifiedColumn {
            table: Self::NAME,
            column,
        }
    }

    /// Addresses the column `C` through this alias.
    ///
    /// Alias markers are zero-sized values, so the method only fixes the column
    /// referenced through the alias and returns the matching [`AliasedCol`]
    /// marker:
    ///
    /// ```
    /// use rurqe_core::schema::{AliasedCol, Column, TableAlias};
    ///
    /// rurqe_core::table! {
    ///     User => "users" {
    ///         UserId => "id": i64,
    ///     }
    /// }
    ///
    /// rurqe_core::table_alias! {
    ///     /// The `u` alias.
    ///     U1 => "u"
    /// }
    ///
    /// let aliased: AliasedCol<U1, UserId> = U1.col(UserId);
    /// let through_column: AliasedCol<U1, UserId> = UserId.alias::<U1>();
    /// ```
    fn col<C: Column>(self, _column: C) -> AliasedCol<Self, C> {
        AliasedCol::new()
    }
}

/// Declares a table alias marker type together with its SQL name.
///
/// ```
/// rurqe_core::table_alias! {
///     /// The `o` alias.
///     OrderAlias => "o"
/// }
///
/// use rurqe_core::schema::TableAlias;
///
/// assert_eq!(OrderAlias::NAME, "o");
/// ```
#[macro_export]
macro_rules! table_alias {
    (
        $(#[$alias_meta:meta])*
        $alias:ident => $alias_name:literal
    ) => {
        $(#[$alias_meta])*
        #[allow(dead_code)]
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub struct $alias;

        impl $crate::schema::TableAlias for $alias {
            const NAME: &'static str = $alias_name;
        }
    };
}

/// The table `T` referenced under the alias `A`.
///
/// The marker keeps the schema identity of `T` — so its columns still belong to
/// `T` — while rendering `"table" AS "alias"` in `FROM` and `JOIN` clauses.
///
/// ```text
/// SELECT "u"."id" FROM "users" AS "u"
/// ```
pub struct Aliased<T, A>(PhantomData<fn() -> (T, A)>);

impl<T: Table, A: TableAlias> Table for Aliased<T, A> {
    const NAME: &'static str = T::NAME;

    fn table_ref() -> TableRef {
        TableRef::aliased(T::NAME, A::NAME)
    }

    fn column_expr(column: &'static str) -> crate::expr::ExprKind {
        crate::expr::ExprKind::QualifiedColumn {
            table: A::NAME,
            column,
        }
    }
}

/// The column `C` referenced through the table alias `A`.
///
/// The column keeps its Rust type and owning table, while its SQL reference
/// becomes `"alias"."column"`, which keeps self-joins and repeated table
/// references unambiguous.
pub struct AliasedCol<A, C>(PhantomData<fn() -> (A, C)>);

impl<A: TableAlias, C: Column> AliasedCol<A, C> {
    /// Creates the aliased column marker for the column `C` under the alias `A`.
    pub fn new() -> Self {
        AliasedCol(PhantomData)
    }
}

impl<A: TableAlias, C: Column> Default for AliasedCol<A, C> {
    fn default() -> Self {
        Self::new()
    }
}

impl<A: TableAlias, C: Column> Column for AliasedCol<A, C> {
    type Table = Aliased<C::Table, A>;
    type RustType = C::RustType;

    const NAME: &'static str = C::NAME;

    fn expr_kind() -> crate::expr::ExprKind {
        crate::expr::ExprKind::QualifiedColumn {
            table: A::NAME,
            column: C::NAME,
        }
    }
}
