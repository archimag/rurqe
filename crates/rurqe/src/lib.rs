//! RURQE: Rust Universal Relational Query Engine.
//!
//! An ergonomic, strictly type-safe SQL query builder and relational execution engine for Rust.

use rurqe_core::dml::{DeleteQuery, InsertQuery, Unfiltered, UpdateQuery};
use rurqe_core::{Dialect, SelectQuery, Table};

pub use rurqe_core as core;

#[cfg(feature = "macros")]
pub use rurqe_macros as macros;

#[cfg(feature = "macros")]
pub use rurqe_macros::{cols, dto, raw_sql, select, sql, SqlType};

pub use rurqe_core::{
    cte, cte_named, dense_rank, lag, lead, rank, row_number, Window, WindowSpec,
};
pub use rurqe_core::{impl_sql_binary_newtype, impl_sql_text_newtype};
pub use rurqe_core::query::TableSelection;
pub use rurqe_core::schema::Next;
pub use rurqe_core::{GroupByInScope, SelectionInScope};

/// Generates a module pinning every query entry point to one dialect version.
///
/// The `postgres` and `sqlite` modules use the macro to expose their
/// version-pinned modules (`pg14` … `pg16` and `sqlite3_0` … `sqlite3_35`).
/// A generated module mirrors its dialect module: `select_from`, `insert_into`,
/// `update`, and `delete_from` take a table value, the `SelectFrom` extension
/// trait starts a query from a selection, and the nested `prelude` re-exports
/// the shared prelude with the pinned entry points taking precedence, so a
/// single glob import pins the whole query surface to one engine version.
///
/// The generated module also defines the [`Error`] and `Result` aliases of the
/// pinned dialect, so an execution helper can be written without naming the
/// driver adapter:
///
/// ```text
/// use rurqe::postgres::pg16::prelude::*;
///
/// select_from(Users).select(cols!(UsersId)).build();
/// ```
#[cfg(any(feature = "postgres", feature = "sqlite"))]
#[macro_export]
macro_rules! pin_dialect_version {
    (
        $(#[$meta:meta])*
        $vis:vis mod $version:ident => $dialect:ty, $error:ty;
    ) => {
        $(#[$meta])*
        $vis mod $version {
            use super::*;

            use $crate::core::dml::{DeleteQuery, InsertQuery, Unfiltered, UpdateQuery};
            use $crate::core::{SelectQuery, Table};

            /// The error type produced by queries pinned to this dialect version.
            pub type Error = $crate::core::query::ExecutionError<$error>;

            /// A [`Result`](core::result::Result) defaulting to [`Error`].
            ///
            /// The error parameter keeps the alias usable where a different
            /// error type is carried, so it never shadows the standard
            /// two-parameter `Result`.
            pub type Result<T, E = Error> = ::core::result::Result<T, E>;

            /// Starts a `SELECT` over `table` with the dialect version pinned.
            pub fn select_from<T: Table>(table: T) -> SelectQuery<$dialect, T, ()> {
                SelectQuery::new().from_table(table)
            }

            /// Starts an `INSERT` into `table` with the dialect version pinned.
            pub fn insert_into<T: Table>(table: T) -> InsertQuery<$dialect, T, (), ()> {
                InsertQuery::into_table(table)
            }

            /// Starts an `UPDATE` of `table` with the dialect version pinned.
            ///
            /// The statement starts unfiltered, so it still requires
            /// `.where_(...)` or `.allow_all()` before it can be built.
            pub fn update<T: Table>(table: T) -> UpdateQuery<$dialect, T, Unfiltered, ()> {
                UpdateQuery::table(table)
            }

            /// Starts a `DELETE` from `table` with the dialect version pinned.
            ///
            /// The statement starts unfiltered, so it still requires
            /// `.where_(...)` or `.allow_all()` before it can be built.
            pub fn delete_from<T: Table>(table: T) -> DeleteQuery<$dialect, T, Unfiltered, ()> {
                DeleteQuery::from_table(table)
            }

            /// Extension trait enabling `.from(table)` on selections with the
            /// dialect version pinned.
            pub trait SelectFrom<T: Table, Positions>: Sized {
                /// The row type produced by the query.
                type Row;

                /// Starts a `SELECT` query over `table` projecting `self`.
                fn from(self, table: T) -> SelectQuery<$dialect, T, Self::Row>;
            }

            impl<S, T, Positions> SelectFrom<T, Positions> for S
            where
                T: Table,
                S: $crate::core::query::SelectionFor<T, Positions> + Send + Sync + 'static,
            {
                type Row = <S as $crate::core::query::SelectionFor<T, Positions>>::Row;

                fn from(self, table: T) -> SelectQuery<$dialect, T, Self::Row> {
                    SelectQuery::new()
                        .from_table(table)
                        .select::<S, Positions>(self)
                }
            }

            /// The shared facade prelude with the version-pinned entry points.
            pub mod prelude {
                pub use $crate::prelude::*;

                pub use super::{
                    delete_from, insert_into, select_from, update, Error, Result, SelectFrom,
                };
            }
        }
    };
}

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

    /// The error type produced by PostgreSQL queries and adapters.
    ///
    /// It lifts a driver-level [`PgError`] into the shared
    /// [`ExecutionError`](rurqe_core::query::ExecutionError), so a single alias
    /// covers backend failures, row decoding failures, and row-count mismatches.
    pub type Error = rurqe_core::query::ExecutionError<PgError>;

    /// A [`Result`](core::result::Result) defaulting to the PostgreSQL [`Error`].
    ///
    /// The error parameter keeps the alias usable where a different error type
    /// is carried, so it never shadows the standard two-parameter `Result`.
    pub type Result<T, E = Error> = ::core::result::Result<T, E>;

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

    /// Extension trait enabling `.from(table)` on selections in PostgreSQL queries.
    pub trait PostgresSelectFrom<T: Table, Positions>: Sized {
        /// The row type produced by the query.
        type Row;

        /// Starts a `SELECT` query over `table` projecting `self`.
        fn from(self, table: T) -> SelectQuery<Postgres, T, Self::Row>;
    }

    impl<S, T, Positions> PostgresSelectFrom<T, Positions> for S
    where
        T: Table,
        S: rurqe_core::query::SelectionFor<T, Positions> + Send + Sync + 'static,
    {
        type Row = <S as rurqe_core::query::SelectionFor<T, Positions>>::Row;

        fn from(self, table: T) -> SelectQuery<Postgres, T, Self::Row> {
            SelectQuery::<Postgres>::new()
                .from_table(table)
                .select::<S, Positions>(self)
        }
    }

    /// The shared facade prelude with PostgreSQL entry points.
    ///
    /// The prelude re-exports the whole facade prelude and replaces its
    /// dialect-generic entry points with PostgreSQL ones, so `select_from`,
    /// `insert_into`, `update`, and `delete_from` take a table value and never
    /// need a dialect annotation.
    pub mod prelude {
        pub use crate::prelude::*;

        pub use super::{
            delete_from, insert_into, select_from, update, Error, PostgresSelectFrom, Result,
        };
    }

    pin_dialect_version! {
        /// Query entry points pinned to PostgreSQL 14.
        pub mod pg14 => Postgres<Pg14>, PgError;
    }

    pin_dialect_version! {
        /// Query entry points pinned to PostgreSQL 15.
        pub mod pg15 => Postgres<Pg15>, PgError;
    }

    pin_dialect_version! {
        /// Query entry points pinned to PostgreSQL 16.
        pub mod pg16 => Postgres<Pg16>, PgError;
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

    /// The error type produced by SQLite queries and adapters.
    ///
    /// It lifts a driver-level [`rusqlite::Error`] into the shared
    /// [`ExecutionError`](rurqe_core::query::ExecutionError), so a single alias
    /// covers backend failures, row decoding failures, and row-count mismatches.
    pub type Error = rurqe_core::query::ExecutionError<rusqlite::Error>;

    /// A [`Result`](core::result::Result) defaulting to the SQLite [`Error`].
    ///
    /// The error parameter keeps the alias usable where a different error type
    /// is carried, so it never shadows the standard two-parameter `Result`.
    pub type Result<T, E = Error> = ::core::result::Result<T, E>;

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

    /// Extension trait enabling `.from(table)` on selections in SQLite queries.
    pub trait SqliteSelectFrom<T: Table, Positions>: Sized {
        /// The row type produced by the query.
        type Row;

        /// Starts a `SELECT` query over `table` projecting `self`.
        fn from(self, table: T) -> SelectQuery<Sqlite, T, Self::Row>;
    }

    impl<S, T, Positions> SqliteSelectFrom<T, Positions> for S
    where
        T: Table,
        S: rurqe_core::query::SelectionFor<T, Positions> + Send + Sync + 'static,
    {
        type Row = <S as rurqe_core::query::SelectionFor<T, Positions>>::Row;

        fn from(self, table: T) -> SelectQuery<Sqlite, T, Self::Row> {
            SelectQuery::<Sqlite>::new()
                .from_table(table)
                .select::<S, Positions>(self)
        }
    }

    /// The shared facade prelude with SQLite entry points.
    ///
    /// The prelude re-exports the whole facade prelude and replaces its
    /// dialect-generic entry points with SQLite ones, so `select_from`,
    /// `insert_into`, `update`, and `delete_from` take a table value and never
    /// need a dialect annotation.
    pub mod prelude {
        pub use crate::prelude::*;

        pub use super::{
            delete_from, insert_into, select_from, update, Error, Result, SqliteSelectFrom,
        };
    }

    pin_dialect_version! {
        /// Query entry points pinned to SQLite 3.0.
        pub mod sqlite3_0 => Sqlite<Sqlite3_0>, rusqlite::Error;
    }

    pin_dialect_version! {
        /// Query entry points pinned to SQLite 3.24.
        pub mod sqlite3_24 => Sqlite<Sqlite3_24>, rusqlite::Error;
    }

    pin_dialect_version! {
        /// Query entry points pinned to SQLite 3.35.
        pub mod sqlite3_35 => Sqlite<Sqlite3_35>, rusqlite::Error;
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
        LagLeadFn, OrderDirection, PartitionBy, QueryBackend, QueryBackendExt, RowDecodeError,
        SelectQuery, SelectTarget, Selection, SqlCompatible, SqlVal, SupportsDistinctOn,
        SupportsOnConflict, SupportsReturning, Table, TableInScope, ToSqlVal, Window, WindowExpr,
        WindowExprKind, WindowFn, WindowSpec,
    };

    pub use rurqe_core::{GroupByInScope, SelectionInScope};

    pub use rurqe_core::query::TableSelection;

    pub use rurqe_core::schema::Next;

    pub use rurqe_core::table_alias;

    pub use rurqe_core::{impl_sql_binary_newtype, impl_sql_text_newtype};

    pub use rurqe_core::dialect::SupportsLateralJoin;

    pub use rurqe_core::expr::{
        avg, coalesce, count, count_all, max, min, sum, CaseBranch, CaseBuilder, Expression,
        StringExpression, Subquery, SubqueryOp,
    };

    pub use rurqe_core::query::{JoinKind, Joined, LeftJoined, Nullable, SetOpKind, TableScope};

    pub use rurqe_core::query::{lateral, CrossLateralJoined, Lateral, LeftLateralJoined};

    pub use rurqe_core::schema::{Aliased, AliasedCol, ExprScope, TableAlias};

    pub use rurqe_core::dml::insert::{
        InsertColumns, InsertColumnsTarget, InsertSource, InsertedSelect,
    };

    pub use rurqe_core::dml::{
        AssignColumn, AssignValue, Assignment, AssignmentSet, AssignmentsPresent, ColumnAssign,
        DeleteQuery, Filtered, HasAssignments, InsertQuery, NoAssignments, Unfiltered, UpdateQuery,
    };

    #[cfg(feature = "macros")]
    pub use rurqe_macros::{cols, dto, raw_sql, select, sql, SqlType};

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
