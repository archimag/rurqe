//! PostgreSQL dialect implementation and runtime adapters for RURQE.

pub use rurqe_core;
#[cfg(feature = "migrate")]
pub use rurqe_migrate;

mod backend;
mod dialect;
mod expressions;
#[cfg(feature = "migrate")]
pub mod introspect;
#[cfg(feature = "migrate")]
pub mod migrate;

pub use dialect::{Pg14, Pg15, Pg16, PgVersion, Postgres};
pub use expressions::{
    plainto_tsquery, to_tsquery, to_tsvector, JsonKey, PgArrayElements, PgArrayExpressions, PgJson,
    PgJsonExpressions, PgJsonValue, PgRegexExpressions, PgTextSearchExpressions, PgTextSearchValue,
    PgTextValue,
};

#[cfg(feature = "tokio-postgres")]
pub use backend::async_conn::{PgAsyncConnection, PgAsyncTransaction};
#[cfg(feature = "tokio-postgres")]
pub use backend::async_scope::{PgAsyncScope, ScopeFuture};
#[cfg(all(feature = "deadpool-postgres", feature = "tokio-postgres"))]
pub use backend::pools::PgAsyncPool;
#[cfg(all(feature = "r2d2", feature = "postgres"))]
pub use backend::pools::{PgPool, PgPoolError, PgPooledConnection};
#[cfg(feature = "postgres")]
pub use backend::sync_conn::PgConnection;
#[cfg(any(feature = "postgres", feature = "tokio-postgres"))]
pub use backend::types::PgError;
