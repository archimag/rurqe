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
//! once per process, so plaintext is never silent.

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
///
/// This build links no TLS backend, so `postgres::NoTls` stays the only
/// connector and the transport policy is enforced in front of the driver
/// instead of being left to it:
///
/// * `sslmode=require`, `verify-ca`, and `verify-full` are refused, because the
///   requirement cannot be honoured and silently downgrading it would send
///   credentials and data in cleartext;
/// * every other mode — including the libpq default `prefer` — is announced once
///   per process, so an unencrypted session is never silent;
/// * `sslmode=disable` states the intent explicitly and stays silent.
fn connect_postgres(url: &str) -> Result<postgres::Client, CliError> {
    let ssl_mode = SslMode::parse(url)?;

    if ssl_mode.forbids_cleartext() {
        return Err(CliError(format!(
            "`{}` demands TLS (`sslmode={}`), but this build of rurqe-cli links no TLS backend; \
             refusing to send credentials and data in cleartext. Terminate TLS in front of the \
             server (in stunnel or pgbouncer, for example) and connect to that local hop with \
             `sslmode=disable`",
            redact_database_url(url),
            ssl_mode.as_str()
        )));
    }

    announce_cleartext(ssl_mode);

    postgres::Client::connect(url, postgres::NoTls).map_err(|e| {
        let err_str = e.to_string();
        let hint = if err_str.to_lowercase().contains("ssl")
            || err_str.to_lowercase().contains("tls")
        {
            "\nHint: the server refuses the unencrypted session; terminate TLS in front of it and connect to that local hop with `sslmode=disable`."
        } else {
            ""
        };
        CliError(format!(
            "failed to connect to PostgreSQL at `{}`: {e}{hint}",
            redact_database_url(url)
        ))
    })
}

/// The transport security a PostgreSQL connection string requests through `sslmode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SslMode {
    /// `sslmode=disable`: an explicitly unencrypted session.
    Disable,
    /// `sslmode=allow`: libpq settles on an unencrypted session where the server allows it.
    Allow,
    /// `sslmode=prefer`, the default libpq applies when the parameter is absent.
    Prefer,
    /// `sslmode=require`: encryption is mandatory, the peer is not verified.
    Require,
    /// `sslmode=verify-ca`: the server certificate chain is verified.
    VerifyCa,
    /// `sslmode=verify-full`: the chain and the server host name are verified.
    VerifyFull,
}

impl SslMode {
    /// Reads the `sslmode` a connection string requests.
    ///
    /// Both spellings libpq accepts are understood — a URL query parameter
    /// (`?sslmode=require`) and a keyword/value parameter (`sslmode=require`,
    /// quoted or not) — and an absent parameter defaults to [`SslMode::Prefer`],
    /// exactly as libpq does.
    fn parse(conninfo: &str) -> Result<Self, CliError> {
        let Some(value) = ssl_mode_value(conninfo) else {
            return Ok(SslMode::Prefer);
        };

        match value.to_ascii_lowercase().as_str() {
            "disable" => Ok(SslMode::Disable),
            "allow" => Ok(SslMode::Allow),
            "prefer" => Ok(SslMode::Prefer),
            "require" => Ok(SslMode::Require),
            "verify-ca" => Ok(SslMode::VerifyCa),
            "verify-full" => Ok(SslMode::VerifyFull),
            other => Err(CliError(format!(
                "unrecognised `sslmode={other}` in `{}`: expected `disable`, `allow`, `prefer`, \
                 `require`, `verify-ca`, or `verify-full`",
                redact_database_url(conninfo)
            ))),
        }
    }

    /// The spelling libpq accepts.
    fn as_str(self) -> &'static str {
        match self {
            SslMode::Disable => "disable",
            SslMode::Allow => "allow",
            SslMode::Prefer => "prefer",
            SslMode::Require => "require",
            SslMode::VerifyCa => "verify-ca",
            SslMode::VerifyFull => "verify-full",
        }
    }

    /// Whether the mode leaves no room for an unencrypted session.
    fn forbids_cleartext(self) -> bool {
        matches!(
            self,
            SslMode::Require | SslMode::VerifyCa | SslMode::VerifyFull
        )
    }
}

/// The `sslmode` value a connection string carries, if any.
///
/// The key is matched in any case and only where a parameter name can start, so
/// a database named `sslmode=...` inside a path is not mistaken for a parameter.
fn ssl_mode_value(conninfo: &str) -> Option<&str> {
    const KEY: &str = "sslmode=";

    // ASCII lowercasing keeps every byte offset and char boundary intact, so the
    // indexes found in the lowered copy address the original string.
    let lowered = conninfo.to_ascii_lowercase();
    let mut search_from = 0;

    while let Some(offset) = lowered.get(search_from..)?.find(KEY) {
        let key_start = search_from + offset;
        let starts_parameter = match conninfo[..key_start].chars().next_back() {
            None => true,
            Some(c) => c.is_whitespace() || matches!(c, '?' | '&' | ';' | '\''),
        };

        if starts_parameter {
            let value = conninfo.get(key_start + KEY.len()..)?;

            // libpq allows any value to be quoted, so a quoted `sslmode` is read
            // up to its closing quote instead of the next separator.
            if let Some(quoted) = value.strip_prefix('\'') {
                let end = quoted.find('\'').unwrap_or(quoted.len());
                return quoted.get(..end);
            }

            let end = value
                .find(|c: char| c.is_whitespace() || matches!(c, '&' | '#' | ';' | '\''))
                .unwrap_or(value.len());
            return value.get(..end);
        }

        search_from = key_start + KEY.len();
    }

    None
}

/// Announces an unencrypted PostgreSQL session once per process.
///
/// `sslmode=disable` states the intent explicitly and stays silent; every other
/// accepted mode — including the libpq default `prefer` — would have negotiated
/// TLS, so the downgrade is reported instead of gaining the session silently.
fn announce_cleartext(ssl_mode: SslMode) {
    if ssl_mode == SslMode::Disable {
        return;
    }

    static WARNED: std::sync::Once = std::sync::Once::new();
    WARNED.call_once(|| {
        eprintln!(
            "warning: connecting to PostgreSQL without TLS (`sslmode={}`); credentials and data \
             travel in cleartext. Terminate TLS in front of the server and connect to that local \
             hop with `sslmode=disable`, or set `sslmode=disable` to acknowledge the plaintext path",
            ssl_mode.as_str()
        );
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_absent_sslmode_keeps_the_libpq_default() {
        assert_eq!(
            SslMode::parse("postgres://app@db.internal:5432/app").expect("a valid mode"),
            SslMode::Prefer
        );
        assert_eq!(
            SslMode::parse("host=db.internal user=app").expect("a valid mode"),
            SslMode::Prefer
        );
    }

    #[test]
    fn both_spellings_of_the_parameter_are_understood() {
        assert_eq!(
            SslMode::parse("postgres://db/app?sslmode=require").expect("a valid mode"),
            SslMode::Require
        );
        assert_eq!(
            SslMode::parse("postgres://db/app?connect_timeout=5&sslmode=Verify-Ca")
                .expect("a valid mode"),
            SslMode::VerifyCa
        );
        assert_eq!(
            SslMode::parse("host=db sslmode=verify-full user=app").expect("a valid mode"),
            SslMode::VerifyFull
        );
        assert_eq!(
            SslMode::parse("host=db sslmode='disable'").expect("a valid mode"),
            SslMode::Disable
        );
    }

    #[test]
    fn only_the_verified_free_modes_forbid_cleartext() {
        for mode in [SslMode::Require, SslMode::VerifyCa, SslMode::VerifyFull] {
            assert!(
                mode.forbids_cleartext(),
                "`{}` must forbid cleartext",
                mode.as_str()
            );
        }

        for mode in [SslMode::Disable, SslMode::Allow, SslMode::Prefer] {
            assert!(
                !mode.forbids_cleartext(),
                "`{}` may stay unencrypted",
                mode.as_str()
            );
        }
    }

    #[test]
    fn unknown_modes_are_rejected_without_leaking_the_password() {
        let error = SslMode::parse("postgres://app:s3cret@db/app?sslmode=maybe")
            .expect_err("an unknown mode is rejected");

        assert!(error.to_string().contains("sslmode=maybe"), "got {error}");
        assert!(!error.to_string().contains("s3cret"), "got {error}");
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
