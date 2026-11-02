//! RURQE: Rust Universal Relational Query Engine.
//!
//! An ergonomic, strictly type-safe SQL query builder and relational execution engine for Rust.
//!
//! # PostgreSQL feature matrix
//!
//! The PostgreSQL surface is split in two halves. The dialect half — the
//! `postgres` module with `Postgres`, `Pg14`, `Pg15`, and `Pg16`, together with
//! the dialect items of every prelude — is compiled by any PostgreSQL adapter
//! feature, so a typed query can always be named, composed, and built. The
//! adapter half is compiled only by the feature that owns the driver:
//!
//! | Feature | Driver items it adds |
//! |---|---|
//! | `postgres` | `PgConnection`, over the synchronous `postgres` driver |
//! | `tokio-postgres` | `PgAsyncConnection`, over the asynchronous `tokio-postgres` driver |
//! | `deadpool-postgres` | `PgAsyncPool`, over the asynchronous driver |
//! | `r2d2` | `PgPool`, over the synchronous driver |
//!
//! Every one of them enables the dialect surface, and none of them pulls the
//! other drivers into the dependency graph, so a build that enables a single
//! adapter still names the dialect:
//!
//! ```text
//! cargo build -p rurqe --no-default-features --features tokio-postgres
//! ```

use rurqe_core::dml::{DeleteQuery, InsertQuery, Unfiltered, UpdateQuery};
use rurqe_core::query::NoProjection;
use rurqe_core::{Dialect, SelectQuery, Table};

/// The `rurqe-core` crate, re-exported as the anchor of every proc-macro expansion.
///
/// A procedural macro cannot name its own facade the way a declarative macro
/// names it with `$crate`, so the expansions of [`rurqe_macros`] address the core
/// crate through this re-export — `::rurqe::__core::expr::SqlVal`, for instance.
/// A crate that depends only on `rurqe` therefore compiles `select!`, `dto!`,
/// `sql!`/`raw_sql!`, and `#[derive(SqlType)]` without adding `rurqe-core` to its
/// own manifest, and renaming this re-export breaks them.
///
/// The anchor deliberately avoids the name `core`: a glob import of the crate
/// root binds every public item, and an item named `core` would take precedence
/// over the built-in `core` crate, silently rebinding every `core::…` path
/// written in the importing module. The `__core` spelling keeps the anchor
/// reachable to the macro expansions while staying a name no importing module
/// writes by accident.
pub use rurqe_core as __core;

#[cfg(feature = "macros")]
pub use rurqe_macros as macros;

#[cfg(feature = "macros")]
pub use rurqe_macros::{cols, dto, raw_sql, select, sql, SqlType};

/// Bakes the migration scripts of a directory into the calling binary.
///
/// The macro is compiled by the `macros` feature and expands into the embedded
/// collection of the `migrate` feature, so it is reachable exactly where both are
/// enabled:
///
/// ```text
/// use rurqe::prelude::*;
///
/// let embedded = embed_migrations!()?;
/// ```
///
/// The migrations directory resolves the way the command line resolves its own:
/// from the `rurqe.toml` found above the manifest of the calling crate, or from
/// the path the call names. See the `embed_migrations!` documentation of the
/// macro crate for the resolution order and for the files the collection carries.
#[cfg(all(feature = "macros", feature = "migrate"))]
pub use rurqe_macros::embed_migrations;

pub use rurqe_core::dml::{excluded, Excluded, ExecutableDelete, ExecutableUpdate};
/// The typed expression surface, re-exported under its own name.
///
/// The module hosts the expression nodes, the call-site predicate traits, and
/// `outer`, the constructor that declares a reference to a column of the
/// statement enclosing the subquery it appears in.
pub use rurqe_core::expr;
pub use rurqe_core::expr::{RowExpr, RowValues};
pub use rurqe_core::query::{LockedQuery, Query, TableSelection};
pub use rurqe_core::{
    cte, cte_named, dense_rank, lag, lead, rank, row_number, BuildError, Window, WindowSpec,
};
pub use rurqe_core::{impl_sql_binary_newtype, impl_sql_text_newtype};
pub use rurqe_core::{GroupByInScope, SelectionInScope};

/// The doc-test catalogue of the facade entry points.
///
/// The positive and `compile_fail` examples verifying the compile-time
/// guarantees of the entry points live in this module, so the facade stays a
/// readable module of re-exports and entry points. The module exists only while
/// rustdoc compiles the doc-tests, so the whole catalogue still runs with
/// `cargo test --doc` and nothing of it reaches a normal build.
#[cfg(doctest)]
mod docs;

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
///
/// A module that also re-exports its driver crate states the extra items, so the
/// dialect module and its version-pinned submodules are generated by one macro:
///
/// ```text
/// pin_dialect_version! {
///     pub mod postgres => Postgres, PgError;
///     items {
///         pub use rurqe_postgres::*;
///     }
/// }
/// ```
///
/// The dialect type and the driver error are resolved where the module is
/// written: a module without extra items names them relative to its parent
/// module and imports that scope, while a module with extra items resolves them
/// from those items, which is why it needs no glob import of its own.
// The macro is compiled with any feature that generates a pinned dialect
// module, which includes every PostgreSQL adapter and not only the synchronous
// driver.
#[cfg(any(
    feature = "postgres",
    feature = "tokio-postgres",
    feature = "deadpool-postgres",
    feature = "r2d2",
    feature = "sqlite"
))]
#[macro_export]
macro_rules! pin_dialect_version {
    // The items shared by both module shapes, expanded inside the module.
    (@module => $dialect:ty, $error:ty;) => {
        use $crate::__core::dml::{DeleteQuery, InsertQuery, Unfiltered, UpdateQuery};
        use $crate::__core::query::{HasProjection, NoProjection};
        use $crate::__core::{SelectQuery, Table};

        /// The error type produced by queries pinned to this dialect version.
        pub type Error = $crate::__core::query::ExecutionError<$error>;

        /// A [`Result`](core::result::Result) defaulting to [`Error`].
        ///
        /// The error parameter keeps the alias usable where a different
        /// error type is carried, so it never shadows the standard
        /// two-parameter `Result`.
        pub type Result<T, E = Error> = ::core::result::Result<T, E>;

        /// Starts a `SELECT` over `table` with the dialect version pinned.
        ///
        /// The query carries no projection yet, so `.select(...)` must follow
        /// before it can be built.
        pub fn select_from<T: Table>(table: T) -> SelectQuery<$dialect, T, (), NoProjection> {
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
            fn from(self, table: T) -> SelectQuery<$dialect, T, Self::Row, HasProjection>;
        }

        impl<S, T, Positions> SelectFrom<T, Positions> for S
        where
            T: Table,
            S: $crate::__core::query::SelectionFor<T, Positions, $dialect>
                + Send
                + Sync
                + 'static,
        {
            type Row = <S as $crate::__core::query::SelectionFor<T, Positions, $dialect>>::Row;

            fn from(self, table: T) -> SelectQuery<$dialect, T, Self::Row, HasProjection> {
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
    };
    (
        $(#[$meta:meta])*
        $vis:vis mod $version:ident => $dialect:ty, $error:ty;
    ) => {
        $(#[$meta])*
        $vis mod $version {
            // The dialect type and the driver error are named relative to this
            // module's parent, so that scope has to be imported here.
            use super::*;

            $crate::pin_dialect_version! {
                @module => $dialect, $error;
            }
        }
    };
    (
        $(#[$meta:meta])*
        $vis:vis mod $version:ident => $dialect:ty, $error:ty;
        items { $($extra:item)* }
    ) => {
        $(#[$meta])*
        $vis mod $version {
            // The extra items re-export the driver crate, so the dialect type and
            // the driver error resolve from them instead of the parent module.
            $($extra)*

            $crate::pin_dialect_version! {
                @module => $dialect, $error;
            }
        }
    };
}

// The dialect surface is compiled as soon as any PostgreSQL adapter feature is
// enabled: bundling the crate must never become a precondition for naming
// `Postgres` or one of its version markers.
#[cfg(any(
    feature = "postgres",
    feature = "tokio-postgres",
    feature = "deadpool-postgres",
    feature = "r2d2"
))]
crate::pin_dialect_version! {
    /// PostgreSQL dialect, version markers, and the runtime driver adapters.
    ///
    /// The module is compiled with any PostgreSQL adapter feature, so a build
    /// that enables only `tokio-postgres`, `deadpool-postgres`, or `r2d2` still
    /// names the dialect and its version markers; the driver items inside it
    /// stay behind the feature that owns them.
    ///
    /// The module mirrors the `rurqe-postgres` crate, so paths such as
    /// `rurqe::postgres::PgConnection` stay reachable, and adds PostgreSQL-pinned
    /// query entry points together with a [`prelude`] that re-exports the shared
    /// facade prelude, so a single glob import makes queries dialect-free:
    ///
    /// ```text
    /// use rurqe::postgres::prelude::*;
    /// ```
    pub mod postgres => Postgres, PgError;

    items {
        pub use rurqe_postgres::*;

        crate::pin_dialect_version! {
            /// Query entry points pinned to PostgreSQL 14.
            pub mod pg14 => Postgres<Pg14>, PgError;
        }

        crate::pin_dialect_version! {
            /// Query entry points pinned to PostgreSQL 15.
            pub mod pg15 => Postgres<Pg15>, PgError;
        }

        crate::pin_dialect_version! {
            /// Query entry points pinned to PostgreSQL 16.
            pub mod pg16 => Postgres<Pg16>, PgError;
        }
    }
}

#[cfg(feature = "migrate")]
pub use rurqe_migrate as migrate;

#[cfg(feature = "sqlite")]
crate::pin_dialect_version! {
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
    pub mod sqlite => Sqlite, rusqlite::Error;

    items {
        pub use rurqe_sqlite::*;

        crate::pin_dialect_version! {
            /// Query entry points pinned to SQLite 3.0.
            pub mod sqlite3_0 => Sqlite<Sqlite3_0>, rusqlite::Error;
        }

        crate::pin_dialect_version! {
            /// Query entry points pinned to SQLite 3.24.
            pub mod sqlite3_24 => Sqlite<Sqlite3_24>, rusqlite::Error;
        }

        crate::pin_dialect_version! {
            /// Query entry points pinned to SQLite 3.25.
            pub mod sqlite3_25 => Sqlite<Sqlite3_25>, rusqlite::Error;
        }

        crate::pin_dialect_version! {
            /// Query entry points pinned to SQLite 3.30.
            pub mod sqlite3_30 => Sqlite<Sqlite3_30>, rusqlite::Error;
        }

        crate::pin_dialect_version! {
            /// Query entry points pinned to SQLite 3.35.
            pub mod sqlite3_35 => Sqlite<Sqlite3_35>, rusqlite::Error;
        }

        crate::pin_dialect_version! {
            /// Query entry points pinned to SQLite 3.38.
            ///
            /// The first release that always bundles the JSON functions, so a
            /// nested collection (`nest_many` / `nest_one`) is reachable from it.
            pub mod sqlite3_38 => Sqlite<Sqlite3_38>, rusqlite::Error;
        }
    }
}

/// Starts a `SELECT` statement over the table `T` for the dialect `D`.
///
/// ```
/// use rurqe::prelude::*;
///
/// table!(
///     /// The `users` table.
///     User => "users" {
///         UserId => "id": i64,
///     }
/// );
///
/// let built = select_from::<Postgres, User>()
///     .select(cols!(UserId))
///     .build()
///     .expect("valid query");
///
/// // The named selection exposes the field it declares as the output column of
/// // the projection, while the column reference inside the expression keeps the
/// // bare name as long as the statement addresses a single relation; it is
/// // qualified only once the statement spans several relations.
/// assert_eq!(built.sql, "SELECT \"id\" AS \"user_id\" FROM \"users\"");
/// ```
///
/// The query records no projection until `select` is called, and only a
/// projected query is renderable, so building it right after the table is
/// introduced is rejected at compile time. Every invariant a projection, a
/// filter, a grouping, a window specification, a row lock, or a nested
/// statement has to satisfy is catalogued as a doc-test in the private `docs`
/// module, which runs with `cargo test --doc`.
pub fn select_from<D: Dialect, T: Table>() -> SelectQuery<D, T, (), NoProjection> {
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
/// table!(
///     /// The `users` table.
///     User => "users" {
///         UserId => "id": i64,
///         UserName => "name": String,
///     }
/// );
///
/// let built = insert_into::<Postgres, User>()
///     .values((UserId::set(1i64), UserName::set("root")))
///     .build()
///     .expect("valid query");
///
/// assert_eq!(
///     built.sql,
///     "INSERT INTO \"users\" (\"id\", \"name\") VALUES ($1, $2)"
/// );
/// ```
///
/// The statement becomes renderable only once it records a row source, a row
/// source known to be empty yields a `Result` instead of the query, and an
/// upsert reads the row the statement tried to insert through the `EXCLUDED`
/// pseudo-relation. The compile-fail catalogue of these rules lives in the
/// private `docs` module and runs with `cargo test --doc`.
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
/// table!(
///     /// The `users` table.
///     User => "users" {
///         UserId => "id": i64,
///         UserName => "name": String,
///     }
/// );
///
/// let built = update::<Postgres, User>()
///     .set(UserName::set("root"))
///     .where_(Expr::column::<UserId>().eq(1i64))
///     .build()
///     .expect("valid query");
///
/// assert_eq!(
///     built.sql,
///     "UPDATE \"users\" SET \"name\" = $1 WHERE (\"id\" = $2)"
/// );
/// ```
///
/// Assignments and `RETURNING` projections are statically tied to the mutated
/// table; the compile-fail catalogue of these typestate rules lives in the
/// private `docs` module and runs with `cargo test --doc`.
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
/// table!(
///     /// The `users` table.
///     User => "users" {
///         UserId => "id": i64,
///     }
/// );
///
/// let built = delete_from::<Postgres, User>()
///     .where_(Expr::column::<UserId>().eq(1i64))
///     .build()
///     .expect("valid query");
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
/// table!(
///     /// The `users` table.
///     User => "users" {
///         UserId => "id": i64,
///     }
/// );
///
/// let built = SelectQuery::<Postgres>::new()
///     .from::<User>()
///     .select((UserId,))
///     .where_(Expr::column::<UserId>().eq(7i64))
///     .build()
///     .expect("valid query");
///
/// assert_eq!(
///     built.sql,
///     "SELECT \"id\" FROM \"users\" WHERE (\"id\" = $1)"
/// );
/// ```
///
/// The prelude also brings the row value traits into scope, so a tuple of
/// columns compares as one composite value — the shape a keyset cursor is
/// walked with; the catalogue of those comparisons lives in the private
/// `docs` module and runs with `cargo test --doc`.
///
/// The prelude also names the flat query shapes: [`Query`] is a `SELECT` that
/// projects its rows and requests no lock, [`LockedQuery`] is one that claims its
/// rows until the transaction ends, and [`ExecutableUpdate`] and
/// [`ExecutableDelete`] are mutations whose filter is already settled.
///
/// The typestates of the builders keep a clause order the engine refuses out of
/// reach, so the rejected arrangements are compile errors rather than
/// diagnostics from a running database. PostgreSQL accepts neither a statement
/// that deduplicates its rows and locks them at the same time, which is why
/// `distinct_on` and `for_update` exclude each other in both orders:
///
/// ```compile_fail
/// use rurqe::prelude::*;
///
/// table!(
///     User => "users" {
///         UserId => "id": i64,
///     }
/// );
///
/// fn main() {
///     // Fails to compile: a statement that deduplicated its rows cannot lock
///     // them afterwards.
///     let _ = select_from::<Postgres, User>()
///         .select((UserId,))
///         .distinct_on(UserId)
///         .for_update();
/// }
/// ```
///
/// ```compile_fail
/// use rurqe::prelude::*;
///
/// table!(
///     User => "users" {
///         UserId => "id": i64,
///     }
/// );
///
/// fn main() {
///     // Fails to compile: a statement that already locked its rows cannot
///     // start deduplicating them.
///     let _ = select_from::<Postgres, User>()
///         .select((UserId,))
///         .for_update()
///         .distinct_on(UserId);
/// }
/// ```
///
/// The `DISTINCT` and `FILTER (WHERE ...)` clauses belong to the aggregate call
/// itself, so the aggregate helpers accept them while a value combined from the
/// call does not:
///
/// ```compile_fail
/// use rurqe::prelude::*;
///
/// table!(
///     Order => "orders" {
///         OrderTotal => "total": f64,
///     }
/// );
///
/// fn main() {
///     // Fails to compile: the sum is combined with a constant, so the
///     // expression no longer ends with the aggregate call and takes no
///     // `FILTER` clause.
///     let _ = (sum(OrderTotal) + 1.0f64).filter_where(OrderTotal.gt(0.0f64));
/// }
/// ```
pub mod prelude {
    pub use crate::{delete_from, insert_into, select_from, update};

    pub use rurqe_core::{
        cte, cte_named, dense_rank, lag, lead, rank, row_number, table, AggregateFunc, BinaryOp,
        BuildError, BuiltQuery, Column, ColumnBelongsTo, Cte, DerivedTable, Dialect,
        ExecutableQuery, ExecutionError, Expr, ExprKind, FromRow, FromSqlValue, FunctionOperand,
        GroupBy, IntoExpr, IsolationLevel, LagLeadFn, OrderDirection, PartitionBy, QueryBackend,
        QueryBackendExt, QueryTerminals, RowDecodeError, SelectQuery, SelectTarget, Selection,
        SqlArrayElement, SqlCompatible, SqlVal, SupportsDistinctOn, SupportsOnConflict,
        SupportsReturning, Table, TableInScope, TerminalBackend, ToSqlVal, Window, WindowExpr,
        WindowExprKind, WindowFn, WindowSpec,
    };

    pub use rurqe_core::query::NullsOrder;

    pub use rurqe_core::{GroupByInScope, SelectionInScope};

    pub use rurqe_core::query::TableSelection;

    pub use rurqe_core::query::{LockedQuery, Query};

    pub use rurqe_core::dml::{ExecutableDelete, ExecutableUpdate};

    pub use rurqe_core::nested::{
        json_object, nest_many, nest_one, nested_field, MultisetRow, NestMany, NestOne, RowObject,
    };

    #[cfg(feature = "serde_json")]
    pub use rurqe_core::nested::{json_to_sql_value, sql_val_to_json};

    pub use rurqe_core::table_alias;

    pub use rurqe_core::{impl_sql_binary_newtype, impl_sql_text_newtype};

    pub use rurqe_core::dialect::{
        SupportsAggregateFilter, SupportsAliasedInsertTarget, SupportsDeleteUsing,
        SupportsDistinctFrom, SupportsFullOuterJoin, SupportsGreatest, SupportsJsonAggregation,
        SupportsLateralJoin, SupportsNullsOrdering, SupportsRowLocking, SupportsUpdateFrom,
        SupportsWindowFunctions,
    };

    pub use rurqe_core::expr::{
        avg, avg_distinct, coalesce, count, count_all, count_distinct, greatest, is_distinct_from,
        is_not_distinct_from, least, max, min, nullif, outer, sum, sum_distinct, AnySubquery,
        CaseBranch, CaseBuilder, Expression, InSubquery, IntoExprList, LockClause, LockStrength,
        LockWait, RowExpr, RowValues, SqlCastTarget, StringExpression, Subquery, SubqueryOp,
        WindowFrame, WindowFrameBound, WindowFrameKind,
    };

    pub use rurqe_core::query::{
        AsyncQueryBackend, AsyncQueryBackendExt, Compound, CrossJoined, FullOuterJoined,
        HasProjection, JoinKind, Joined, LeftJoined, Locked, NoLock, NoProjection, Nullable,
        ProjectionPresent, SetOpKind, TableScope, TailClosed,
    };

    pub use rurqe_core::query::{lateral, CrossLateralJoined, Lateral, LeftLateralJoined};

    pub use rurqe_core::schema;

    pub use rurqe_core::schema::{Aliased, AliasedCol, ExprScope, TableAlias};

    pub use rurqe_core::dml::insert::{
        InsertColumns, InsertColumnsTarget, InsertSource, InsertedSelect,
    };

    pub use rurqe_core::dml::{
        excluded, AssignColumn, AssignValue, Assignment, AssignmentSet, AssignmentsPresent,
        ColumnAssign, DeleteQuery, EmptyRows, Excluded, Filtered, HasAssignments, InsertQuery,
        NoAssignments, Unfiltered, UpdateQuery,
    };

    #[cfg(feature = "macros")]
    pub use rurqe_macros::{cols, dto, raw_sql, select, sql, SqlType};

    /// Bakes the migration scripts of a directory into the calling binary.
    ///
    /// The macro requires both the `macros` feature that compiles it and the
    /// `migrate` feature whose collection its expansion constructs.
    #[cfg(all(feature = "macros", feature = "migrate"))]
    pub use rurqe_macros::embed_migrations;

    // The dialect items reach the prelude with any PostgreSQL adapter feature, so
    // an asynchronous or pooled build names `Postgres` without the synchronous
    // driver; the driver items below keep their own feature.
    #[cfg(any(
        feature = "postgres",
        feature = "tokio-postgres",
        feature = "deadpool-postgres",
        feature = "r2d2"
    ))]
    pub use rurqe_postgres::{
        JsonKey, Pg14, Pg15, Pg16, PgArrayElements, PgArrayExpressions,
        /* PgConnection is imported below */ PgJson, PgJsonExpressions, PgJsonValue,
        PgRegexExpressions, PgTextSearchExpressions, PgTextSearchValue, PgTextValue, PgVersion,
        Postgres,
    };

    /// The synchronous `postgres` adapter, compiled by the `postgres` feature and
    /// by `r2d2`, which pools that driver.
    #[cfg(any(feature = "postgres", feature = "r2d2"))]
    pub use rurqe_postgres::PgConnection;

    #[cfg(feature = "tokio-postgres")]
    pub use rurqe_postgres::{PgAsyncConnection, PgAsyncScope};

    #[cfg(any(feature = "postgres", feature = "tokio-postgres"))]
    pub use rurqe_postgres::PgError;

    #[cfg(feature = "sqlite")]
    pub use rurqe_sqlite::{
        Sqlite, Sqlite3_0, Sqlite3_24, Sqlite3_25, Sqlite3_30, Sqlite3_35, Sqlite3_38,
        SqliteConnection, SqliteVersion,
    };

    #[cfg(feature = "r2d2")]
    pub use rurqe_postgres::{PgPool, PgPoolError};

    #[cfg(feature = "r2d2")]
    pub use rurqe_sqlite::{SqlitePool, SqlitePoolError};

    #[cfg(feature = "deadpool-postgres")]
    pub use rurqe_postgres::PgAsyncPool;

    #[cfg(feature = "migrate")]
    pub use rurqe_migrate::{
        warn_if_in_tokio_runtime, Checksum, EmbeddedMigrations, Migration, MigrationKind,
        MigrationStatus, MigrationStatusKind, MigrationVersion, NoopSleeper, SchemaCatalog,
        Sleeper, ThreadSleeper,
    };
}
