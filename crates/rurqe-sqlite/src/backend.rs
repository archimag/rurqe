//! Runtime adapters executing RURQE-built queries through SQLite.
//!
//! The adapters mirror the layering of the PostgreSQL crate: `types` translates a
//! typed [`SqlVal`](rurqe_core::expr::SqlVal) into a driver parameter and a driver
//! value back into a typed one, `rusqlite` owns the connection adapter, `refusal`
//! classifies the constraint refusals the driver reports, and `pools` exposes the
//! `r2d2` pool. Every layer is gated by the dependency it needs, so a
//! build without the driver compiles an empty module instead.

#[cfg(feature = "r2d2")]
mod pools;
#[cfg(feature = "rusqlite")]
mod refusal;
#[cfg(feature = "rusqlite")]
mod rusqlite;
#[cfg(feature = "rusqlite")]
mod types;

#[cfg(feature = "r2d2")]
pub use self::pools::{SqlitePool, SqlitePoolError};
#[cfg(feature = "rusqlite")]
pub use self::refusal::SqliteRefusalExt;
#[cfg(feature = "rusqlite")]
pub use self::rusqlite::SqliteConnection;
