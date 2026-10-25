//! Database connection lifecycle for the CLI.
//!
//! [`resolve_database_url`] picks the connection target from the flag or the
//! `DATABASE_URL` environment variable, and [`connect`] opens the matching
//! engine handle so every command shares a single connection code path.
//!
//! PostgreSQL sessions are opened without TLS because this build links no TLS
//! backend, so the transport policy is enforced in front of the driver instead of
//! being left to it:
//!
//! * a connection string demanding TLS (`sslmode=require`) is refused, because
//!   the requirement cannot be honoured;
//! * reaching a remote host in cleartext is refused unless the connection string
//!   spells `sslmode=disable`, because the libpq default `prefer` would have
//!   negotiated TLS and is exactly the silent downgrade this policy rejects;
//! * a loopback address and a Unix-domain socket are accepted in every mode,
//!   because those bytes never leave the machine.
//!
//! Every accepted session is announced once per process — at `info!` when a
//! remote host was explicitly acknowledged, at `debug!` for a local transport —
//! so plaintext is never silent. The mode and the host list come from the typed
//! `postgres::Config` the driver's own parser produces, never from a
//! hand-written scan of the URL.

use postgres::config::SslMode;
use postgres::error::SqlState;

use crate::target::DatabaseTarget;

use super::args::CliError;
use super::redact::{host_name, redact_database_url};

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
/// connector and [`enforce_transport_policy`] decides which sessions may be
/// opened in cleartext at all: a demand for TLS and an implicit downgrade to a
/// remote host are both refused, while a local transport and an explicit
/// `sslmode=disable` are accepted.
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

    enforce_transport_policy(&config, url)?;
    announce_cleartext(config.get_ssl_mode(), remote_host(&config).as_deref());

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

/// Refuses a transport this build cannot secure without the operator stating the intent.
///
/// This build links no TLS backend, so every PostgreSQL session is cleartext. The
/// policy therefore separates the transports that never leave the machine from the
/// ones that do:
///
/// * `sslmode=require` is refused, because the requirement cannot be honoured and
///   gaining the session anyway would send credentials and data in cleartext;
/// * any other mode on a remote host is refused unless it is `sslmode=disable`,
///   because the libpq default `prefer` would have negotiated TLS and is exactly
///   the silent downgrade this policy rejects — cleartext to a remote server is
///   accepted only when the connection string spells it out;
/// * a loopback address and a Unix-domain socket are accepted in every mode,
///   because those bytes never reach a network.
///
/// The host list and the mode come from the driver's own `postgres::Config`, and
/// the refusal carries the redacted connection string, so a refused attempt never
/// echoes a password.
fn enforce_transport_policy(config: &postgres::Config, url: &str) -> Result<(), CliError> {
    let ssl_mode = config.get_ssl_mode();

    if ssl_mode == SslMode::Require {
        return Err(CliError(format!(
            "`{}` demands TLS (`sslmode=require`), but this build of rurqe-cli links no TLS \
             backend; refusing to send credentials and data in cleartext. Terminate TLS in front \
             of the server (in stunnel, pgbouncer, or a cloud proxy, for example) and connect to \
             that local hop with `sslmode=disable`, or connect over a Unix-domain socket",
            redact_database_url(url)
        )));
    }

    let Some(host) = remote_host(config) else {
        return Ok(());
    };

    // `disable` is the one spelling that states the choice, so a remote host
    // reached with it is acknowledged instead of refused.
    if ssl_mode == SslMode::Disable {
        return Ok(());
    }

    Err(CliError(format!(
        "`{}` would reach the remote host `{host}` in cleartext (`sslmode={}`), because this \
         build of rurqe-cli links no TLS backend; refusing to send credentials and data \
         unencrypted. Terminate TLS in front of the server and connect to that local hop, or \
         acknowledge the plaintext path explicitly with `sslmode=disable`",
        redact_database_url(url),
        ssl_mode_name(ssl_mode)
    )))
}

/// The first host of a connection string that is not reachable over a local transport.
///
/// A connection string may name several hosts and the driver tries them in order,
/// so a single remote entry is enough to make the whole session potentially
/// remote.
fn remote_host(config: &postgres::Config) -> Option<String> {
    config
        .get_hosts()
        .iter()
        .find(|host| !is_local_host(host))
        .map(host_name)
}

/// Whether a host specification stays on the machine.
///
/// A Unix-domain socket never leaves the machine, and a TCP host counts as local
/// only when its name is `localhost` or a loopback address. The classification is
/// textual: resolving a name would need a network round trip of its own, so a DNS
/// alias of `127.0.0.1` stays classified as remote and demands the explicit
/// `sslmode=disable` acknowledgement.
fn is_local_host(host: &postgres::config::Host) -> bool {
    match host {
        postgres::config::Host::Tcp(name) => is_loopback_name(name),
        _ => true,
    }
}

/// Whether a host name spells a loopback destination.
///
/// The driver may keep an IPv6 literal in the brackets the connection string
/// carries, so the brackets are removed before the address is parsed. An
/// IPv4-mapped IPv6 address (`::ffff:127.0.0.1`) reaches IPv4 loopback through
/// its mapped form.
fn is_loopback_name(name: &str) -> bool {
    if name.eq_ignore_ascii_case("localhost") {
        return true;
    }

    let bare = name
        .strip_prefix('[')
        .and_then(|rest| rest.strip_suffix(']'))
        .unwrap_or(name);

    match bare.parse::<std::net::IpAddr>() {
        Ok(std::net::IpAddr::V4(address)) => address.is_loopback(),
        Ok(std::net::IpAddr::V6(address)) => {
            address.is_loopback()
                || address
                    .to_ipv4_mapped()
                    .is_some_and(|mapped| mapped.is_loopback())
        }
        Err(_) => false,
    }
}

/// Announces an accepted unencrypted PostgreSQL session once per process.
///
/// The policy has already refused every implicit downgrade, so a session is
/// cleartext only because it stays on the machine or because the operator wrote
/// `sslmode=disable` for a remote host. The first case is the normal shape of a
/// developer workstation and is reported at `debug!`; the second one is a
/// deliberate exception and is recorded at `info!`. The notices travel through
/// the `tracing` facade, which leaves the sink to the application embedding this
/// crate.
fn announce_cleartext(ssl_mode: SslMode, remote: Option<&str>) {
    static ACKNOWLEDGED: std::sync::Once = std::sync::Once::new();
    static LOCAL: std::sync::Once = std::sync::Once::new();

    if let Some(host) = remote {
        ACKNOWLEDGED.call_once(|| {
            tracing::info!(
                host = %host,
                ssl_mode = %ssl_mode_name(ssl_mode),
                "connecting to a remote PostgreSQL host in cleartext, as the connection string \
                 requests with `sslmode=disable`; a build with a TLS backend would encrypt this \
                 session"
            );
        });
        return;
    }

    if ssl_mode == SslMode::Disable {
        return;
    }

    LOCAL.call_once(|| {
        tracing::debug!(
            ssl_mode = %ssl_mode_name(ssl_mode),
            "connecting to a local PostgreSQL transport without TLS; this build links no TLS \
             backend and the session never leaves the machine"
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
        assert!(
            message.contains("invalid PostgreSQL connection string"),
            "got {message}"
        );
        // The diagnostic carries the fixed placeholder instead of the parameter
        // text, so the offending value — and the password beside it — never
        // reach the log.
        assert!(
            message.contains("<unparseable PostgreSQL connection string>"),
            "got {message}"
        );
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

    /// Parses a connection string through the driver's own parser.
    fn config_of(url: &str) -> postgres::Config {
        url.parse().expect("a valid connection string")
    }

    #[test]
    fn a_remote_host_without_a_stated_mode_is_refused() {
        let url = "postgres://app:s3cret@db.internal:5432/app";
        let error = enforce_transport_policy(&config_of(url), url)
            .expect_err("the libpq default would silently downgrade to cleartext");

        let message = error.to_string();
        assert!(message.contains("db.internal"), "got {message}");
        assert!(message.contains("sslmode=prefer"), "got {message}");
        assert!(message.contains("sslmode=disable"), "got {message}");
        assert!(!message.contains("s3cret"), "got {message}");
    }

    #[test]
    fn an_acknowledged_cleartext_remote_host_is_accepted() {
        for url in [
            "postgres://app@db.internal/app?sslmode=disable",
            "host=db.internal user=app sslmode=disable dbname=app",
        ] {
            enforce_transport_policy(&config_of(url), url)
                .expect("`sslmode=disable` states the cleartext intent");
        }
    }

    #[test]
    fn a_local_transport_is_accepted_without_naming_the_mode() {
        for url in [
            "postgres://app@localhost/app",
            "postgres://app@127.0.0.1:5432/app",
            "postgres://app@127.9.8.7/app",
            "postgres://app@[::1]:5432/app",
            "host=/var/run/postgresql user=app dbname=app",
        ] {
            enforce_transport_policy(&config_of(url), url)
                .unwrap_or_else(|error| panic!("`{url}` stays on the machine: {error}"));
        }
    }

    #[test]
    fn one_remote_host_is_enough_to_refuse_a_multi_host_string() {
        let url = "host=localhost,db.internal user=app dbname=app";
        let error = enforce_transport_policy(&config_of(url), url)
            .expect_err("the driver may land on the remote host");

        assert!(error.to_string().contains("db.internal"), "got {error}");
    }

    #[test]
    fn host_classification_separates_loopback_from_the_network() {
        for local in [
            "localhost",
            "LOCALHOST",
            "127.0.0.1",
            "127.4.3.2",
            "::1",
            "[::1]",
            "::ffff:127.0.0.1",
        ] {
            assert!(is_loopback_name(local), "`{local}` is loopback");
        }

        for remote in [
            "db.internal",
            "0.0.0.0",
            "10.0.0.5",
            "::2",
            "localhost.example",
        ] {
            assert!(!is_loopback_name(remote), "`{remote}` leaves the machine");
        }
    }
}
