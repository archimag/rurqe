//! RURQE: Rust Universal Relational Query Engine.
//!
//! An ergonomic, strictly type-safe SQL query builder and relational execution engine for Rust.

pub use rurqe_core as core;

#[cfg(feature = "macros")]
pub use rurqe_macros as macros;

#[cfg(feature = "postgres")]
pub use rurqe_postgres as postgres;

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
    pub use rurqe_core::{
        table, BinaryOp, BuiltQuery, Column, ColumnBelongsTo, Dialect, Expr, ExprKind, FromRow,
        FromSqlValue, OrderDirection, SelectQuery, Selection, SqlCompatible, SqlVal,
        SupportsDistinctOn, SupportsOnConflict, SupportsReturning, Table, TableInScope,
    };

    #[cfg(feature = "postgres")]
    pub use rurqe_postgres::{Pg14, Pg15, Pg16, PgVersion, Postgres};
}
