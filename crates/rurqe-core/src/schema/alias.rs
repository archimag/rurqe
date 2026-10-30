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
/// The marker-only form declares an alias that mirrors no table and only names
/// the alias itself:
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
///
/// The table-bound form also mirrors the columns of the aliased table, so the
/// alias addresses them exactly like the table does, and every field stays
/// typed as the marker of that column under the alias:
///
/// ```
/// use rurqe_core::schema::{AliasedCol, Column, TableAlias};
///
/// rurqe_core::table! {
///     User => "users" {
///         UserId => "id": i64,
///         UserEmail => "email": String,
///     }
/// }
///
/// rurqe_core::table_alias! {
///     /// The `u` alias.
///     U1 => User as "u"
/// }
///
/// assert_eq!(U1::NAME, "u");
///
/// // `U1.UserId` addresses the column through the alias, as `User.UserId`
/// // addresses it through the table.
/// let through_alias: AliasedCol<U1, UserId> = U1.UserId;
/// assert_eq!(<AliasedCol<U1, UserId> as Column>::NAME, "id");
/// ```
///
/// The table-bound form reaches the columns through the declaration of the
/// table, so it is written in the module that declares the table.
#[macro_export]
macro_rules! table_alias {
    (
        $(#[$alias_meta:meta])*
        $alias:ident => $table:ident as $alias_name:literal
    ) => {
        $table! { @alias [$(#[$alias_meta])*] $alias, $alias_name }
    };
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

/// Declares the accessor mirror of a table alias.
///
/// [`table_alias!`](crate::table_alias) reaches this declaration through the
/// hidden macro [`table!`](crate::table) emits, which is the only place that
/// knows the columns of the aliased table. The column list arrives as the
/// `(marker, SQL name)` pairs the table declared, and every pair becomes a
/// field of the alias accessor typed as that column under the alias, followed
/// by the constant of the same name that exposes the accessor itself.
///
/// The invocation is an implementation detail of the two public macros and is
/// not part of the stable surface.
#[doc(hidden)]
#[macro_export]
macro_rules! __rurqe_alias_decl {
    (
        [$(($column:ident, $column_name:literal)),* $(,)?]
        [$(#[$alias_meta:meta])*]
        $alias:ident,
        $alias_name:literal
    ) => {
        $(#[$alias_meta])*
        #[allow(dead_code, non_snake_case)]
        pub struct $alias {
            $(
                #[allow(non_snake_case)]
                pub $column: $crate::schema::AliasedCol<$alias, $column>,
            )*
        }

        #[allow(dead_code, non_upper_case_globals)]
        pub const $alias: $alias = $alias {
            $(
                $column: $crate::schema::AliasedCol::<$alias, $column>::new(),
            )*
        };

        impl $crate::schema::TableAlias for $alias {
            const NAME: &'static str = $alias_name;
        }
    };
}

/// The table `T` referenced under the alias `A`.
///
/// The marker keeps the schema identity of `T` — so its columns still belong to
/// `T` — while rendering `"table" AS "alias"` in `FROM` and `JOIN` clauses. The
/// schema qualifier of the aliased table passes through, so an alias of a table
/// of a secondary schema renders `"auth"."users" AS "u"`.
///
/// ```text
/// SELECT "u"."id" FROM "users" AS "u"
/// ```
pub struct Aliased<T, A>(PhantomData<fn() -> (T, A)>);

impl<T: Table, A: TableAlias> Aliased<T, A> {
    /// Creates the marker referencing `T` under the alias `A`.
    ///
    /// Both markers are zero-sized, so the constructor carries no data and the
    /// value compiles away; it exists to hand the aliased table to the
    /// value-based builders such as `SelectQuery::from_table` and
    /// `SelectQuery::inner_join`.
    pub fn new() -> Self {
        Aliased(PhantomData)
    }
}

impl<T: Table, A: TableAlias> Default for Aliased<T, A> {
    fn default() -> Self {
        Self::new()
    }
}
impl<T: Table, A: TableAlias> Table for Aliased<T, A> {
    const NAME: &'static str = T::NAME;

    const SCHEMA: Option<&'static str> = T::SCHEMA;

    fn table_ref() -> TableRef {
        TableRef {
            schema: T::SCHEMA,
            name: T::NAME,
            alias: Some(A::NAME),
        }
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
    ///
    /// The constructor is a `const fn`, so the accessor an alias declares can
    /// mirror the accessor of the table it aliases as a constant.
    pub const fn new() -> Self {
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
