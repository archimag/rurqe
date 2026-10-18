//! RURQE: Rust Universal Relational Query Engine.
//!
//! An ergonomic, strictly type-safe SQL query builder and relational execution engine for Rust.

use rurqe_core::dml::{DeleteQuery, InsertQuery, Unfiltered, UpdateQuery};
use rurqe_core::{Dialect, Table};

pub use rurqe_core as core;

#[cfg(feature = "macros")]
pub use rurqe_macros as macros;

#[cfg(feature = "postgres")]
pub use rurqe_postgres as postgres;

/// Starts an `INSERT` statement over the table `T` for the dialect `D`.
///
/// Rows are supplied through `values` or `values_many`, and every assigned value
/// is checked against its column's Rust type at the call site.
///
/// ```
/// use rurqe::prelude::*;
///
/// table! {
///     /// The `users` table.
///     User => "users" {
///         UserId => "id": i64,
///         UserName => "name": String,
///     }
/// }
///
/// let built = insert_into::<Postgres, User>()
///     .values((UserId::set(1i64), UserName::set("root")))
///     .build();
///
/// assert_eq!(
///     built.sql,
///     "INSERT INTO \"users\" (\"id\", \"name\") VALUES ($1, $2)"
/// );
/// ```
pub fn insert_into<D: Dialect, T: Table>() -> InsertQuery<D, T, (), ()> {
    InsertQuery::new()
}

/// Starts an `UPDATE` statement over the table `T` for the dialect `D`.
///
/// The builder starts in the unfiltered typestate, where `build` stays
/// unavailable: either call `where_` to restrict the affected rows or
/// `allow_all` to confirm a full-table mutation explicitly.
///
/// ```
/// use rurqe::prelude::*;
///
/// table! {
///     /// The `users` table.
///     User => "users" {
///         UserId => "id": i64,
///         UserName => "name": String,
///     }
/// }
///
/// let built = update::<Postgres, User>()
///     .set(UserName::set("root"))
///     .where_(Expr::column::<UserId>().eq(1i64))
///     .build();
///
/// assert_eq!(
///     built.sql,
///     "UPDATE \"users\" SET \"name\" = $1 WHERE (\"id\" = $2)"
/// );
/// ```
pub fn update<D: Dialect, T: Table>() -> UpdateQuery<D, T, Unfiltered, ()> {
    UpdateQuery::new()
}

/// Starts a `DELETE` statement over the table `T` for the dialect `D`.
///
/// The builder starts in the unfiltered typestate, where `build` stays
/// unavailable: either call `where_` to restrict the affected rows or
/// `allow_all` to confirm a full-table deletion explicitly.
///
/// ```
/// use rurqe::prelude::*;
///
/// table! {
///     /// The `users` table.
///     User => "users" {
///         UserId => "id": i64,
///     }
/// }
///
/// let built = delete_from::<Postgres, User>()
///     .where_(Expr::column::<UserId>().eq(1i64))
///     .build();
///
/// assert_eq!(built.sql, "DELETE FROM \"users\" WHERE (\"id\" = $1)");
/// ```
pub fn delete_from<D: Dialect, T: Table>() -> DeleteQuery<D, T, Unfiltered, ()> {
    DeleteQuery::new()
}

/// Common imports for declaring schemas, building queries, and decoding rows.
///
/// ```
/// use rurqe::prelude::*;
///
/// table! {
///     /// The `users` table.
///     User => "users" {
///         UserId => "id": i64,
///     }
/// }
///
/// let built = SelectQuery::<Postgres>::new()
///     .from::<User>()
///     .select((UserId,))
///     .where_(Expr::column::<UserId>().eq(7i64))
///     .build();
///
/// assert_eq!(
///     built.sql,
///     "SELECT \"id\" FROM \"users\" WHERE (\"id\" = $1)"
/// );
/// ```
pub mod prelude {
    pub use crate::{delete_from, insert_into, update};

    pub use rurqe_core::{
        table, AggregateFunc, At, BinaryOp, BuiltQuery, Column, ColumnBelongsTo, Dialect, Expr,
        ExprKind, FromRow, FromSqlValue, GroupBy, OrderDirection, SelectQuery, Selection,
        SqlCompatible, SqlVal, SupportsDistinctOn, SupportsOnConflict, SupportsReturning, Table,
        TableInScope,
    };

    pub use rurqe_core::query::{JoinKind, Joined, LeftJoined, Nullable, TableScope};

    pub use rurqe_core::dml::{
        AssignColumn, AssignValue, Assignment, AssignmentSet, ColumnAssign, DeleteQuery, Filtered,
        InsertQuery, Unfiltered, UpdateQuery,
    };

    #[cfg(feature = "macros")]
    pub use rurqe_macros::{dto, select};

    #[cfg(feature = "postgres")]
    pub use rurqe_postgres::{Pg14, Pg15, Pg16, PgVersion, Postgres};
}
