//! Database connection lifecycle for the CLI.
//!
//! [`resolve_database_url`] picks the connection target from the flag or the
//! `DATABASE_URL` environment variable, and [`connect`] opens the matching
//! engine handle so every command shares a single connection code path.
//!
//! PostgreSQL sessions are opened without TLS because this build links no TLS
//! backend. The transport policy is therefore enforced in front of the driver:
//! a connection string demanding TLS is refused instead of being silently
//! downgraded to cleartext, and every accepted unencrypted session is announced
//! once per process, so plaintext is never silent. The requested mode comes from
//! the typed `SslMode` the connection-string parser yields, never from a
//! hand-written scan of the URL.

use postgres::config::SslMode;
use postgres::error::SqlState;

use crate::target::DatabaseTarget;

use super::args::CliError;
use super::redact::redact_database_url;

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
    match DatabaseTarget::parse(url) {
        DatabaseTarget::Postgres => Ok(Connection::Postgres(connect_postgres(url)?)),
        DatabaseTarget::SqliteMemory => open_sqlite(None, url).map(Connection::Sqlite),
        DatabaseTarget::SqliteFile(path) => {
            open_sqlite(Some(path.as_str()), url).map(Connection::Sqlite)
        }
    }
}

/// Opens a SQLite connection, in memory when the target names no file.
fn open_sqlite(path: Option<&str>, url: &str) -> Result<rusqlite::Connection, CliError> {
    let conn = match path {
        Some(path) => rusqlite::Connection::open(path),
        None => rusqlite::Connection::open_in_memory(),
    };

    conn.map_err(|e| {
        CliError(format!(
            "failed to open SQLite database at `{}`: {e}",
            redact_database_url(url)
        ))
    })
}

/// Connects to PostgreSQL from a connection string.
///
/// This build links no TLS backend, so `postgres::NoTls` stays the only
/// connector and the transport policy is enforced in front of the driver
/// instead of being left to it:
///
/// * `sslmode=require` is refused, because the requirement cannot be honoured
///   and silently downgrading it would send credentials and data in cleartext;
/// * every other accepted mode — including the libpq default `prefer` — is
///   announced once per process, so an unencrypted session is never silent;
/// * `sslmode=disable` states the intent explicitly and stays silent.
///
/// The connection string is parsed by the driver's own `postgres::Config`
/// parser, so the transport mode comes from a typed `SslMode` and a malformed
/// string is reported before any connection is attempted. A server that refuses
/// the session before authentication is reported with a remediation hint keyed
/// on its typed `SqlState`, never on the wording of the driver message.
fn connect_postgres(url: &str) -> Result<postgres::Client, CliError> {
    let config = url.parse::<postgres::Config>().map_err(|error| {
        CliError(format!(
            "invalid PostgreSQL connection string `{}`: {error}",
            redact_database_url(url)
        ))
    })?;

    let ssl_mode = config.get_ssl_mode();

    if ssl_mode == SslMode::Require {
        return Err(CliError(format!(
            "`{}` demands TLS (`sslmode=require`), but this build of rurqe-cli links no TLS \
             backend; refusing to send credentials and data in cleartext. Terminate TLS in front \
             of the server (in stunnel or pgbouncer, for example) and connect to that local hop \
             with `sslmode=disable`",
            redact_database_url(url)
        )));
    }

    announce_cleartext(ssl_mode);

    config.connect(postgres::NoTls).map_err(|error| {
        let hint = if error.code() == Some(&SqlState::INVALID_AUTHORIZATION_SPECIFICATION) {
            "\nHint: the server refused the session before authentication; if its `pg_hba.conf` \
             requires encrypted connections, terminate TLS in front of the server and connect to \
             that local hop with `sslmode=disable`."
        } else {
            ""
        };

        CliError(format!(
            "failed to connect to PostgreSQL at `{}`: {error}{hint}",
            redact_database_url(url)
        ))
    })
}

/// The libpq spelling of a transport security mode.
///
/// `SslMode` is a non-exhaustive enum, so a mode added by the driver upstream is
/// rendered through its `Debug` spelling instead of failing to compile; this
/// build links no TLS backend, so only the three libpq modes are ever reached.
fn ssl_mode_name(ssl_mode: SslMode) -> String {
    match ssl_mode {
        SslMode::Disable => "disable".to_owned(),
        SslMode::Prefer => "prefer".to_owned(),
        SslMode::Require => "require".to_owned(),
        other => format!("{other:?}").to_lowercase(),
    }
}

/// Announces an unencrypted PostgreSQL session once per process.
///
/// `sslmode=disable` states the intent explicitly and stays silent; every other
/// accepted mode — including the libpq default `prefer` — would have negotiated
/// TLS, so the downgrade is reported instead of gaining the session silently.
/// The notice travels through the `tracing` facade, which leaves the sink to the
/// application embedding this crate.
fn announce_cleartext(ssl_mode: SslMode) {
    if ssl_mode == SslMode::Disable {
        return;
    }

    static WARNED: std::sync::Once = std::sync::Once::new();
    WARNED.call_once(|| {
        tracing::warn!(
            ssl_mode = %ssl_mode_name(ssl_mode),
            "connecting to PostgreSQL without TLS; credentials and data travel in cleartext. \
             Terminate TLS in front of the server and connect to that local hop with \
             `sslmode=disable`, or set `sslmode=disable` to acknowledge the plaintext path"
        );
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_absent_sslmode_keeps_the_libpq_default() {
        let config: postgres::Config = "postgres://app@db.internal:5432/app"
            .parse()
            .expect("a valid connection string");
        assert_eq!(config.get_ssl_mode(), SslMode::Prefer);

        let config: postgres::Config = "host=db.internal user=app"
            .parse()
            .expect("a valid connection string");
        assert_eq!(config.get_ssl_mode(), SslMode::Prefer);
    }

    #[test]
    fn both_spellings_of_the_parameter_are_understood() {
        let config: postgres::Config = "postgres://db/app?sslmode=require"
            .parse()
            .expect("a valid connection string");
        assert_eq!(config.get_ssl_mode(), SslMode::Require);

        let config: postgres::Config = "postgres://db/app?connect_timeout=5&sslmode=prefer"
            .parse()
            .expect("a valid connection string");
        assert_eq!(config.get_ssl_mode(), SslMode::Prefer);

        let config: postgres::Config = "host=db sslmode=disable user=app"
            .parse()
            .expect("a valid connection string");
        assert_eq!(config.get_ssl_mode(), SslMode::Disable);
    }

    #[test]
    fn unknown_modes_are_rejected_without_leaking_the_password() {
        let error = connect_postgres("postgres://app:s3cret@127.0.0.1:1/app?sslmode=maybe")
            .err()
            .expect("an unknown mode is rejected");

        let message = error.to_string();
        assert!(message.contains("sslmode=maybe"), "got {message}");
        assert!(!message.contains("s3cret"), "got {message}");
    }

    #[test]
    fn a_connection_demanding_tls_is_refused_before_the_driver_runs() {
        // `postgres::Client` carries no `Debug` impl, so the error is taken out
        // of the result without the `Result::expect_err` bound.
        let error = connect_postgres("postgres://app:s3cret@127.0.0.1:1/app?sslmode=require")
            .err()
            .expect("a TLS requirement cannot be honoured");

        let message = error.to_string();
        assert!(message.contains("sslmode=require"), "got {message}");
        assert!(message.contains("TLS backend"), "got {message}");
        assert!(!message.contains("s3cret"), "got {message}");
    }
}
