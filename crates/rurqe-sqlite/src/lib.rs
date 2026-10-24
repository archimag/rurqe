//! SQLite dialect implementation and runtime adapters for RURQE.
//!
//! The dialect is parameterised by a type-level engine version, so SQL features
//! introduced by newer SQLite releases exist only for versions that actually
//! support them:
//!
//! - [`Sqlite3_0`] — baseline dialect without `ON CONFLICT` or `RETURNING`.
//! - [`Sqlite3_24`] — adds `ON CONFLICT` upsert syntax.
//! - [`Sqlite3_35`] — adds the `RETURNING` clause and `UPDATE ... FROM`.
//!
//! ```
//! use rurqe_core::dialect::Dialect;
//! use rurqe_sqlite::{Sqlite, Sqlite3_35};
//!
//! assert_eq!(Sqlite::<Sqlite3_35>::quote_identifier("users"), "\"users\"");
//! assert_eq!(Sqlite::<Sqlite3_35>::placeholder(1), "?");
//! ```
//!
//! The dialect lives in `dialect`, and the runtime adapters are split by the
//! dependency they need — the value translation, the `rusqlite` connection, and the
//! `r2d2` pool — exactly like the PostgreSQL crate does.

mod backend;
mod dialect;

pub use rurqe_core;

pub use self::dialect::{
    Sqlite, Sqlite3_0, Sqlite3_24, Sqlite3_25, Sqlite3_30, Sqlite3_35, SqliteVersion,
    SqliteVersionAtLeast,
};

/// A `rusqlite` connection extended with the RURQE execution contract.
#[cfg(feature = "rusqlite")]
pub use self::backend::SqliteConnection;

/// An `r2d2` pool of SQLite connections.
#[cfg(feature = "r2d2")]
pub use self::backend::{SqlitePool, SqlitePoolError};

/// Re-export of the underlying `rusqlite` crate for adapter users.
#[cfg(feature = "rusqlite")]
pub use ::rusqlite;
