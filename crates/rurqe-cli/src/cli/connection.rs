//! Database connection lifecycle for the CLI.
//!
//! [`resolve_database_url`] picks the connection target from the flag or the
//! `DATABASE_URL` environment variable, and [`connect`] opens the matching
//! engine handle so every command shares a single connection code path.

use crate::introspect::{detect_dialect, DatabaseDialect};

use super::args::{redact_database_url, CliError};

/// Resolves the target database URL from the flag or the `DATABASE_URL` environment variable.
pub(super) fn resolve_database_url(supplied: Option<String>) -> Result<String, CliError> {
    supplied
        .or_else(|| std::env::var("DATABASE_URL").ok())
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            CliError(
                "no database URL provided: pass `--database-url <url>` or set the \
                 DATABASE_URL environment variable"
                    .to_owned(),
            )
        })
}

/// A live database connection opened from a URL or filesystem path.
pub(crate) enum Connection {
    /// SQLite database handle.
    Sqlite(rusqlite::Connection),
    /// PostgreSQL client.
    Postgres(postgres::Client),
}

/// Opens a connection to the database identified by `url`, detecting its engine.
///
/// This is the single entry point for scheme stripping, `:memory:` handling and
/// connection error wording, so the CLI commands and catalog introspection can
/// never drift apart.
pub(crate) fn connect(url: &str) -> Result<Connection, CliError> {
    match detect_dialect(Some(url)) {
        DatabaseDialect::Sqlite => Ok(Connection::Sqlite(open_sqlite(url)?)),
        DatabaseDialect::Postgres => Ok(Connection::Postgres(connect_postgres(url)?)),
    }
}

/// Opens a SQLite connection from a URL or plain filesystem path.
fn open_sqlite(url: &str) -> Result<rusqlite::Connection, CliError> {
    let path = url
        .strip_prefix("sqlite://")
        .or_else(|| url.strip_prefix("sqlite:"))
        .unwrap_or(url);

    let conn = if path == ":memory:" || path.is_empty() {
        rusqlite::Connection::open_in_memory()
    } else {
        rusqlite::Connection::open(path)
    };

    conn.map_err(|e| {
        CliError(format!(
            "failed to open SQLite database at `{}`: {e}",
            redact_database_url(url)
        ))
    })
}

/// Connects to PostgreSQL from a connection URL.
fn connect_postgres(url: &str) -> Result<postgres::Client, CliError> {
    postgres::Client::connect(url, postgres::NoTls).map_err(|e| {
        let err_str = e.to_string();
        let hint = if err_str.to_lowercase().contains("ssl")
            || err_str.to_lowercase().contains("tls")
        {
            "\nHint: PostgreSQL server may require SSL/TLS, but the connection was attempted with NoTls."
        } else {
            ""
        };
        CliError(format!(
            "failed to connect to PostgreSQL at `{}`: {e}{hint}",
            redact_database_url(url)
        ))
    })
}
