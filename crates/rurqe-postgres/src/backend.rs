//! PostgreSQL driver execution adapters and shared parameter binding.

#[cfg(any(feature = "postgres", feature = "tokio-postgres"))]
pub mod types;

// The classification of a constraint refusal reads the description the server
// sent, which both adapters record through the same `PgError`, so the module is
// compiled with the binding layer it classifies.
#[cfg(any(feature = "postgres", feature = "tokio-postgres"))]
pub mod refusal;

#[cfg(feature = "tokio-postgres")]
pub mod async_conn;

#[cfg(feature = "tokio-postgres")]
pub mod async_scope;

// The poison protocol is shared by the asynchronous adapters: a connection the
// adapter can no longer prove to be outside a transaction refuses further work
// instead of running it inside an abandoned one. The module is compiled with the
// adapters it serves, so a synchronous-only build carries no unreachable code.
#[cfg(feature = "tokio-postgres")]
pub(crate) mod poison;
#[cfg(feature = "postgres")]
pub mod sync_conn;

// Either pool compiles the module, while each pool inside it is gated on the
// driver it pools: `deadpool-postgres` on the asynchronous adapter and `r2d2`
// on the synchronous one.
#[cfg(any(feature = "deadpool-postgres", feature = "r2d2"))]
pub mod pools;
