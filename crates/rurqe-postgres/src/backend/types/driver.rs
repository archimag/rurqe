//! Driver types shared by the adapters compiled into the crate.
//!
//! The asynchronous and the synchronous driver expose the same wire types, the
//! same error, and the same result row, so the binding layer names them once
//! through this facade instead of picking a driver at every use site. The
//! asynchronous driver is preferred when both are compiled in, since the
//! synchronous one is built on top of it.
/// The synchronous driver re-exports the types of the asynchronous one, which is
/// why the facade falls back to it when only `postgres` is compiled in.
///
/// The `error` module is the other half of the facade: the refusal the server
/// reported is described by the `DbError` of that module, and it is named once
/// here for either driver.
#[cfg(all(feature = "postgres", not(feature = "tokio-postgres")))]
pub(super) use postgres::{error, types, Error, Row};
#[cfg(feature = "tokio-postgres")]
pub(super) use tokio_postgres::{error, types, Error, Row};
