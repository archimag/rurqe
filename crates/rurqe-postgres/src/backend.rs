//! PostgreSQL driver execution adapters and shared parameter binding.

#[cfg(any(feature = "postgres", feature = "tokio-postgres"))]
pub mod types;

#[cfg(feature = "tokio-postgres")]
pub mod async_conn;

#[cfg(feature = "tokio-postgres")]
pub mod async_scope;

#[cfg(feature = "postgres")]
pub mod sync_conn;

// Either pool compiles the module, while each pool inside it is gated on the
// driver it pools: `deadpool-postgres` on the asynchronous adapter and `r2d2`
// on the synchronous one.
#[cfg(any(feature = "deadpool-postgres", feature = "r2d2"))]
pub mod pools;
