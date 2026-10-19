//! RURQE: Rust Universal Relational Query Engine.
//!
//! An ergonomic, strictly type-safe SQL query builder and relational execution engine for Rust.

use rurqe_core::dml::{DeleteQuery, InsertQuery, Unfiltered, UpdateQuery};
use rurqe_core::{Dialect, SelectQuery, Table};

pub use rurqe_core as core;

#[cfg(feature = "macros")]
pub use rurqe_macros as macros;

#[cfg(feature = "macros")]
pub use rurqe_macros::{cols, dto, raw_sql, select, sql};

pub use rurqe_core::{
    cte, cte_named, dense_rank, lag, lead, rank, row_number, Window, WindowSpec,
};
pub use rurqe_core::query::TableSelection;
pub use rurqe_core::schema::Next;
pub use rurqe_core::{GroupByInScope, SelectionInScope};

/// PostgreSQL dialect, version markers, and the runtime driver adapters.
///
/// The module mirrors the `rurqe-postgres` crate, so paths such as
/// `rurqe::postgres::PgConnection` stay reachable, and adds PostgreSQL-pinned
/// query entry points together with a [`prelude`] that re-exports the shared
/// facade prelude, so a single glob import makes queries dialect-free:
///
/// ```text
/// use rurqe::postgres::prelude::*;
/// ```
#[cfg(feature = "postgres")]
pub mod postgres {
    pub use rurqe_postgres::*;

    use rurqe_core::dml::{DeleteQuery, InsertQuery, Unfiltered, UpdateQuery};
    use rurqe_core::{SelectQuery, Table};

    /// Starts a `SELECT` over `table` with the dialect pinned to PostgreSQL.
    ///
    /// Table markers are zero-sized values, so the table is passed as an
    /// argument and neither the table nor the dialect needs a turbofish:
    ///
    /// ```text
    /// select_from(Users).select(cols!(UsersId)).build()
    /// ```
    pub fn select_from<T: Table>(table: T) -> SelectQuery<Postgres, T, ()> {
        SelectQuery::new().from_table(table)
    }

    /// Starts an `INSERT` into `table` with the dialect pinned to PostgreSQL.
    pub fn insert_into<T: Table>(table: T) -> InsertQuery<Postgres, T, (), ()> {
        InsertQuery::into_table(table)
    }

    /// Starts an `UPDATE` of `table` with the dialect pinned to PostgreSQL.
    ///
    /// The statement starts unfiltered, so it still requires `.where_(...)` or
    /// `.allow_all()` before it can be built.
    pub fn update<T: Table>(table: T) -> UpdateQuery<Postgres, T, Unfiltered, ()> {
        UpdateQuery::table(table)
    }

    /// Starts a `DELETE` from `table` with the dialect pinned to PostgreSQL.
    ///
    /// The statement starts unfiltered, so it still requires `.where_(...)` or
    /// `.allow_all()` before it can be built.
    pub fn delete_from<T: Table>(table: T) -> DeleteQuery<Postgres, T, Unfiltered, ()> {
        DeleteQuery::from_table(table)
    }

    /// The shared facade prelude with PostgreSQL entry points.
    ///
    /// The prelude re-exports the whole facade prelude and replaces its
    /// dialect-generic entry points with PostgreSQL ones, so `select_from`,
    /// `insert_into`, `update`, and `delete_from` take a table value and never
    /// need a dialect annotation.
    pub mod prelude {
        pub use crate::prelude::*;

        pub use super::{delete_from, insert_into, select_from, update};
    }
}

#[cfg(feature = "migrate")]
pub use rurqe_migrate as migrate;

/// SQLite dialect, version markers, and the `rusqlite` execution adapter.
///
/// The module mirrors the `rurqe-sqlite` crate, so paths such as
/// `rurqe::sqlite::rusqlite::Connection` stay reachable, and adds SQLite-pinned
/// query entry points together with a [`prelude`] that re-exports the shared
/// facade prelude, so a single glob import makes queries dialect-free:
///
/// ```text
/// use rurqe::sqlite::prelude::*;
/// ```
#[cfg(feature = "sqlite")]
pub mod sqlite {
    pub use rurqe_sqlite::*;

    use rurqe_core::dml::{DeleteQuery, InsertQuery, Unfiltered, UpdateQuery};
    use rurqe_core::{SelectQuery, Table};

    /// Starts a `SELECT` over `table` with the dialect pinned to SQLite.
    ///
    /// Table markers are zero-sized values, so the table is passed as an
    /// argument and neither the table nor the dialect needs a turbofish:
    ///
    /// ```text
    /// select_from(Users).select(cols!(UsersId)).build()
    /// ```
    pub fn select_from<T: Table>(table: T) -> SelectQuery<Sqlite, T, ()> {
        SelectQuery::new().from_table(table)
    }

    /// Starts an `INSERT` into `table` with the dialect pinned to SQLite.
    pub fn insert_into<T: Table>(table: T) -> InsertQuery<Sqlite, T, (), ()> {
        InsertQuery::into_table(table)
    }

    /// Starts an `UPDATE` of `table` with the dialect pinned to SQLite.
    ///
    /// The statement starts unfiltered, so it still requires `.where_(...)` or
    /// `.allow_all()` before it can be built.
    pub fn update<T: Table>(table: T) -> UpdateQuery<Sqlite, T, Unfiltered, ()> {
        UpdateQuery::table(table)
    }

    /// Starts a `DELETE` from `table` with the dialect pinned to SQLite.
    ///
    /// The statement starts unfiltered, so it still requires `.where_(...)` or
    /// `.allow_all()` before it can be built.
    pub fn delete_from<T: Table>(table: T) -> DeleteQuery<Sqlite, T, Unfiltered, ()> {
        DeleteQuery::from_table(table)
    }

    /// The shared facade prelude with SQLite entry points.
    ///
    /// The prelude re-exports the whole facade prelude and replaces its
    /// dialect-generic entry points with SQLite ones, so `select_from`,
    /// `insert_into`, `update`, and `delete_from` take a table value and never
    /// need a dialect annotation.
    pub mod prelude {
        pub use crate::prelude::*;

        pub use super::{delete_from, insert_into, select_from, update};
    }
}

/// Starts a `SELECT` statement over the table `T` for the dialect `D`.
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
/// let built = select_from::<Postgres, User>()
///     .select(cols!(UserId))
///     .build();
///
/// assert_eq!(
///     built.sql,
///     "SELECT \"users\".\"id\" FROM \"users\""
/// );
/// ```
///
/// Projecting a column of a table the query does not select from is rejected at
/// compile time:
///
/// ```compile_fail
/// use rurqe::prelude::*;
///
/// table! {
///     /// The `users` table.
///     User => "users" {
///         UserId => "id": i64,
///     }
/// }
///
/// table! {
///     /// The `orders` table.
///     Order => "orders" {
///         OrderTotal => "total": f64,
///     }
/// }
///
/// // Fails to compile: `OrderTotal` belongs to `Order`, which is not in scope.
/// let _ = select_from::<Postgres, User>().select((UserId, OrderTotal));
/// ```
///
/// Grouping and ordering follow the same rule:
///
/// ```compile_fail
/// use rurqe::prelude::*;
///
/// table! {
///     /// The `users` table.
///     User => "users" {
///         UserId => "id": i64,
///     }
/// }
///
/// table! {
///     /// The `orders` table.
///     Order => "orders" {
///         OrderTotal => "total": f64,
///     }
/// }
///
/// // Fails to compile: `OrderTotal` is not a column of a table in scope.
/// let _ = select_from::<Postgres, User>()
///     .select((UserId,))
///     .group_by(OrderTotal);
/// ```
///
/// ```compile_fail
/// use rurqe::prelude::*;
///
/// table! {
///     /// The `users` table.
///     User => "users" {
///         UserId => "id": i64,
///     }
/// }
///
/// table! {
///     /// The `orders` table.
///     Order => "orders" {
///         OrderTotal => "total": f64,
///     }
/// }
///
/// // Fails to compile: `OrderTotal` is not a column of a table in scope.
/// let _ = select_from::<Postgres, User>()
///     .select((UserId,))
///     .order_by_desc(OrderTotal);
/// ```
pub fn select_from<D: Dialect, T: Table>() -> SelectQuery<D, T, ()> {
    SelectQuery::new().from::<T>()
}

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
///
/// Assignments are statically tied to the mutated table, so a column of another
/// table cannot be set on `User`:
///
/// ```compile_fail
/// use rurqe::prelude::*;
///
/// table! {
///     /// The `users` table.
///     User => "users" {
///         UserId => "id": i64,
///     }
/// }
///
/// table! {
///     /// The `orders` table.
///     Order => "orders" {
///         OrderTotal => "total": f64,
///     }
/// }
///
/// // Fails to compile: `OrderTotal` belongs to `Order`, not to `User`.
/// let _ = update::<Postgres, User>().set(OrderTotal::set(1.0));
/// ```
///
/// `RETURNING` projections are tied to the mutated table as well:
///
/// ```compile_fail
/// use rurqe::prelude::*;
///
/// table! {
///     /// The `users` table.
///     User => "users" {
///         UserId => "id": i64,
///     }
/// }
///
/// table! {
///     /// The `orders` table.
///     Order => "orders" {
///         OrderTotal => "total": f64,
///     }
/// }
///
/// // Fails to compile: `OrderTotal` is not a column of `User`.
/// let _ = update::<Postgres, User>()
///     .set(UserId::set(1i64))
///     .allow_all()
///     .returning((OrderTotal,));
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
    pub use crate::{delete_from, insert_into, select_from, update};

    pub use rurqe_core::{
        cte, cte_named, dense_rank, lag, lead, rank, row_number, table, AggregateFunc, At,
        BinaryOp, BuiltQuery, Column, ColumnBelongsTo, Cte, Dialect, ExecutableQuery,
        ExecutionError, Expr, ExprKind, FromRow, FromSqlValue, GroupBy, IntoExpr, IsolationLevel,
        LagLeadFn, OrderDirection, PartitionBy, QueryBackend, QueryBackendExt, SelectQuery,
        SelectTarget, Selection, SqlCompatible, SqlVal, SupportsDistinctOn, SupportsOnConflict,
        SupportsReturning, Table, TableInScope, Window, WindowExpr, WindowExprKind, WindowFn,
        WindowSpec,
    };

    pub use rurqe_core::{GroupByInScope, SelectionInScope};

    pub use rurqe_core::query::TableSelection;

    pub use rurqe_core::schema::Next;

    pub use rurqe_core::table_alias;

    pub use rurqe_core::dialect::SupportsLateralJoin;

    pub use rurqe_core::expr::{
        avg, coalesce, count, count_all, max, min, sum, CaseBranch, CaseBuilder, Expression,
        StringExpression, Subquery, SubqueryOp,
    };

    pub use rurqe_core::query::{JoinKind, Joined, LeftJoined, Nullable, SetOpKind, TableScope};

    pub use rurqe_core::query::{lateral, CrossLateralJoined, Lateral, LeftLateralJoined};

    pub use rurqe_core::schema::{Aliased, AliasedCol, ExprScope, TableAlias};

    pub use rurqe_core::dml::insert::{InsertColumns, InsertColumnsTarget, InsertedSelect};

    pub use rurqe_core::dml::{
        AssignColumn, AssignValue, Assignment, AssignmentSet, ColumnAssign, DeleteQuery, Filtered,
        InsertQuery, Unfiltered, UpdateQuery,
    };

    #[cfg(feature = "macros")]
    pub use rurqe_macros::{cols, dto, raw_sql, select, sql};

    #[cfg(feature = "postgres")]
    pub use rurqe_postgres::{Pg14, Pg15, Pg16, PgConnection, PgVersion, Postgres};

    #[cfg(feature = "tokio-postgres")]
    pub use rurqe_postgres::{PgAsyncConnection, PgError};

    #[cfg(feature = "sqlite")]
    pub use rurqe_sqlite::{
        Sqlite, Sqlite3_0, Sqlite3_24, Sqlite3_35, SqliteConnection, SqliteVersion,
    };

    #[cfg(feature = "r2d2")]
    pub use rurqe_postgres::{PgPool, PgPoolError};

    #[cfg(feature = "r2d2")]
    pub use rurqe_sqlite::{SqlitePool, SqlitePoolError};

    #[cfg(feature = "deadpool-postgres")]
    pub use rurqe_postgres::PgAsyncPool;

    #[cfg(feature = "migrate")]
    pub use rurqe_migrate::{
        embed_migrations, Checksum, EmbeddedMigrations, Migration, MigrationKind, MigrationStatus,
        MigrationStatusKind, MigrationVersion, SchemaCatalog,
    };
}
