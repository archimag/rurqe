//! PostgreSQL dialect implementation and runtime adapters for RURQE.

pub use rurqe_core;

mod backend;
mod dialect;
mod expressions;

pub use dialect::{Pg14, Pg15, Pg16, PgVersion, Postgres};
pub use expressions::{
    plainto_tsquery, to_tsquery, to_tsvector, JsonKey, PgArrayElements, PgArrayExpressions,
    PgExpressions,
};

#[cfg(feature = "tokio-postgres")]
pub use backend::async_conn::{PgAsyncConnection, PgAsyncTransaction};
#[cfg(feature = "deadpool-postgres")]
pub use backend::pools::PgAsyncPool;
#[cfg(any(feature = "postgres", feature = "tokio-postgres"))]
pub use backend::types::PgError;
#[cfg(feature = "r2d2")]
pub use backend::pools::{PgPool, PgPoolError};
#[cfg(feature = "postgres")]
pub use backend::sync_conn::PgConnection;
