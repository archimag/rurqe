//! PostgreSQL driver execution adapters and shared parameter binding.

#[cfg(any(feature = "postgres", feature = "tokio-postgres"))]
pub mod types;

#[cfg(feature = "tokio-postgres")]
pub mod async_conn;

#[cfg(feature = "postgres")]
pub mod sync_conn;

#[cfg(any(feature = "deadpool-postgres", feature = "r2d2"))]
pub mod pools;
