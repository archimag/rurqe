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
//! * a connection string demanding TLS — `sslmode=require` today, the verified
//!   `verify-ca` / `verify-full` modes as soon as the driver models them, and any
//!   mode a later driver adds — is refused on every host, because the requirement
//!   cannot be honoured;
//! * reaching a remote host in cleartext is refused unless the connection string
//!   spells `sslmode=disable`, because the libpq default `prefer` would have
//!   negotiated TLS and is exactly the silent downgrade this policy rejects;
//! * a loopback address and a Unix-domain socket are accepted in the modes that
//!   demand no TLS, because those bytes never leave the machine.
//!
//! Every accepted session is announced once per process — at `warn!` when a
//! remote host was explicitly acknowledged, at `debug!` for a local transport —
//! so plaintext is never silent. The sink carrying those notices is installed by
//! the shipped binaries in [`crate::install_diagnostics`]. The mode and the host
//! list come from the typed `postgres::Config` the driver's own parser produces;
//! the raw string is consulted only for an `sslmode` spelling that parser rejects
//! outright, and then through `url::form_urlencoded` rather than a hand-written
//! scan.

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
/// string is reported before any connection is attempted. A spelling of `sslmode`
/// the driver does not model — `verify-ca` and `verify-full` in this build — makes
/// the whole string unparseable, so that failure is attributed to the TLS demand it
/// carries instead of to a syntax error the operator cannot act upon. A server that
/// refuses the session before authentication is reported with a remediation hint
/// keyed on its typed `SqlState`, never on the wording of the driver message.
fn connect_postgres(url: &str) -> Result<postgres::Client, CliError> {
    let config = url.parse::<postgres::Config>().map_err(|error| {
        match unmodelled_tls_demand(url) {
            Some(spelling) => tls_unavailable(spelling, url),
            None => CliError(format!(
                "invalid PostgreSQL connection string `{}`: {error}",
                redact_database_url(url)
            )),
        }
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
/// rendered from its `Debug` spelling in the hyphenated form libpq accepts —
/// `VerifyFull` becomes `verify-full`, never `verifyfull` — instead of failing to
/// compile, so a refusal always names the mode the way an operator writes it in a
/// connection string.
fn ssl_mode_name(ssl_mode: SslMode) -> String {
    match ssl_mode {
        SslMode::Disable => "disable".to_owned(),
        SslMode::Prefer => "prefer".to_owned(),
        SslMode::Require => "require".to_owned(),
        other => kebab_case(&format!("{other:?}")),
    }
}

/// Renders a driver `Debug` spelling in the hyphenated form libpq accepts.
///
/// A hyphen is emitted before an upper-case letter that follows a lower-case or
/// digit character, so `VerifyFull` renders as `verify-full` and `VerifyCa` as
/// `verify-ca`; a closing run of capitals stays one word, so a hypothetical
/// `VerifyCA` renders as `verify-ca` as well.
fn kebab_case(debug: &str) -> String {
    let mut rendered = String::with_capacity(debug.len() + 2);
    let mut previous_upper = true;

    for character in debug.chars() {
        if character.is_ascii_uppercase() {
            if !previous_upper {
                rendered.push('-');
            }
            rendered.push(character.to_ascii_lowercase());
            previous_upper = true;
        } else {
            rendered.push(character);
            previous_upper = false;
        }
    }

    rendered
}

/// Whether a transport mode demands TLS this build cannot provide.
///
/// The classification is structural rather than a list of remembered spellings: the
/// two modes that demand no encryption — `disable`, and `prefer`, which merely
/// tolerates TLS — are the only accepted ones, so `require`, the verified
/// `verify-ca` / `verify-full` modes a driver update may start modelling, and any
/// further variant the non-exhaustive enum gains all land in the refusal. An
/// unpinned assumption about the driver's feature set therefore cannot silently
/// reopen the gap.
fn demands_tls(ssl_mode: SslMode) -> bool {
    !matches!(ssl_mode, SslMode::Disable | SslMode::Prefer)
}

/// Refuses a transport this build cannot secure without the operator stating the intent.
///
/// This build links no TLS backend, so every PostgreSQL session is cleartext. The
/// policy therefore separates a transport that demands encryption from the ones that
/// never leave the machine:
///
/// * a mode demanding TLS is refused on every host — a loopback address and a
///   Unix-domain socket included — because the requirement cannot be honoured and
///   gaining the session anyway would send credentials and data in cleartext;
/// * any other mode on a remote host is refused unless it is `sslmode=disable`,
///   because the libpq default `prefer` would have negotiated TLS and is exactly
///   the silent downgrade this policy rejects — cleartext to a remote server is
///   accepted only when the connection string spells it out;
/// * a loopback address and a Unix-domain socket are accepted in the modes that
///   demand no TLS, because those bytes never reach a network.
///
/// The host list and the mode come from the driver's own `postgres::Config`, and
/// the refusal carries the redacted connection string, so a refused attempt never
/// echoes a password.
fn enforce_transport_policy(config: &postgres::Config, url: &str) -> Result<(), CliError> {
    let ssl_mode = config.get_ssl_mode();

    if demands_tls(ssl_mode) {
        return Err(tls_unavailable(&ssl_mode_name(ssl_mode), url));
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

/// The single refusal of every connection string that demands TLS.
///
/// The mode is rendered the way libpq spells it, so both paths reaching this refusal
/// — a mode the driver models and a spelling its parser does not know yet — state one
/// policy and one pair of remedies, and the strictness of the demand never decides
/// the wording: refuse to chat in cleartext when encryption was asked for, terminate
/// TLS in front of the server and connect to that local hop, or acknowledge the
/// plaintext path explicitly with `sslmode=disable`; a Unix-domain socket needs
/// neither.
fn tls_unavailable(ssl_mode: &str, url: &str) -> CliError {
    CliError(format!(
        "`{}` demands TLS (`sslmode={ssl_mode}`), but this build of rurqe-cli links no TLS \
         backend; refusing to send credentials and data in cleartext. Terminate TLS in front of \
         the server and connect to that local hop, or acknowledge the plaintext path explicitly \
         with `sslmode=disable`; a Unix-domain socket needs neither",
        redact_database_url(url)
    ))
}

/// The TLS-demanding spellings libpq defines for the `sslmode` parameter.
///
/// A spelling the linked driver does not model never reaches the typed `SslMode`,
/// which is exactly why it has to be recognised here: the connection string fails
/// parsing otherwise, and the operator would be told the string is malformed instead
/// of being told that the encryption it demands is unavailable.
const TLS_DEMANDING_SPELLINGS: [&str; 3] = ["require", "verify-ca", "verify-full"];

/// The TLS-demanding `sslmode` spelling a connection string asks for while the
/// driver's parser does not model it.
///
/// This is the fallback the parse failure in [`connect_postgres`] consults: a spelling
/// the driver does not know makes the whole string unparseable, so the typed
/// `SslMode` never exists and the policy has nothing to classify. The `sslmode` value
/// is read from the parameters of the string — the query of a URL, or the
/// whitespace-separated keyword/value list handed to the same parser by joining its
/// parameters on `&` — so it comes from a parsed key/value pair instead of a scan of
/// the raw text. [`driver_models_sslmode`] keeps the fallback honest: a spelling the
/// driver understands is reported by the policy, never here, so an unrelated defect
/// in a connection string is not blamed on TLS.
fn unmodelled_tls_demand(url: &str) -> Option<&'static str> {
    let parameters = match url::Url::parse(url) {
        Ok(parsed) => parsed.query().unwrap_or_default().to_owned(),
        Err(_) => url.replace(|character: char| character.is_ascii_whitespace(), "&"),
    };

    url::form_urlencoded::parse(parameters.as_bytes())
        .filter(|(key, _)| key.as_ref() == "sslmode")
        .find_map(|(_, value)| {
            let spelling =
                value.trim_matches(|character: char| character == '\'' || character == '"');

            TLS_DEMANDING_SPELLINGS
                .into_iter()
                .find(|candidate| *candidate == spelling && !driver_models_sslmode(candidate))
        })
}

/// Whether the driver's own parser accepts a mode spelling.
///
/// The probe is a minimal connection string handed to the driver, so a spelling the
/// driver models is reported by the driver rather than by a list kept here.
fn driver_models_sslmode(spelling: &str) -> bool {
    format!("sslmode={spelling}")
        .parse::<postgres::Config>()
        .is_ok()
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
/// security-relevant exception and is recorded at `warn!`, so the sink the shipped
/// binaries install carries it to the operator instead of leaving plaintext
/// silent. The notices travel through the `tracing` facade, which leaves the sink
/// to the application embedding this crate.
fn announce_cleartext(ssl_mode: SslMode, remote: Option<&str>) {
    static ACKNOWLEDGED: std::sync::Once = std::sync::Once::new();
    static LOCAL: std::sync::Once = std::sync::Once::new();

    if let Some(host) = remote {
        ACKNOWLEDGED.call_once(|| {
            tracing::warn!("{}", acknowledged_cleartext_notice(host, ssl_mode));
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

/// The notice announcing a remote session the operator acknowledged in cleartext.
///
/// The wording is built apart from the emission, so the notice names both the host
/// and the mode the session really uses and a regression test can pin them.
fn acknowledged_cleartext_notice(host: &str, ssl_mode: SslMode) -> String {
    format!(
        "connecting to the remote PostgreSQL host `{host}` in cleartext, as the connection string \
         requests with `sslmode={}`; a build with a TLS backend would encrypt this session",
        ssl_mode_name(ssl_mode)
    )
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

    #[test]
    fn an_acknowledged_cleartext_session_is_announced_with_its_host_and_mode() {
        let notice = acknowledged_cleartext_notice("db.internal", SslMode::Disable);

        assert!(notice.contains("db.internal"), "got {notice}");
        assert!(notice.contains("sslmode=disable"), "got {notice}");
        assert!(notice.contains("cleartext"), "got {notice}");
    }

    #[test]
    fn every_mode_that_demands_tls_is_classified_as_unavailable() {
        assert!(demands_tls(SslMode::Require));
        assert!(!demands_tls(SslMode::Prefer));
        assert!(!demands_tls(SslMode::Disable));
    }

    #[test]
    fn the_refusal_names_a_mode_the_way_libpq_spells_it() {
        assert_eq!(ssl_mode_name(SslMode::Require), "require");
        assert_eq!(ssl_mode_name(SslMode::Prefer), "prefer");
        assert_eq!(ssl_mode_name(SslMode::Disable), "disable");

        // A mode a driver update may add is rendered hyphenated, never as the
        // `Debug` spelling.
        assert_eq!(kebab_case("VerifyFull"), "verify-full");
        assert_eq!(kebab_case("VerifyCa"), "verify-ca");
        assert_eq!(kebab_case("VerifyCA"), "verify-ca");
        assert_eq!(kebab_case("Require"), "require");
    }

    #[test]
    fn a_verified_tls_demand_never_opens_a_cleartext_session() {
        // The linked driver models neither `verify-ca` nor `verify-full`, so these
        // strings fail its parser and the refusal is attributed to the TLS demand.
        // A driver that starts modelling them lands in `demands_tls` and is refused
        // with the same wording, so either way the demand never becomes a cleartext
        // session on a Unix-domain socket or a loopback address.
        for url in [
            "host=/var/run/postgresql user=app dbname=app sslmode=verify-full",
            "host=localhost user=app dbname=app sslmode=verify-ca",
            "host=localhost user=app sslmode='verify-full'",
            "postgres://app@localhost/app?sslmode=verify-full",
            "postgres://app@127.0.0.1/app?sslmode=verify-ca",
        ] {
            let error = connect_postgres(url)
                .err()
                .unwrap_or_else(|| panic!("`{url}` demands TLS this build cannot provide"));

            let message = error.to_string();
            assert!(message.contains("TLS backend"), "`{url}`: {message}");
            assert!(
                message.contains("sslmode=verify-"),
                "`{url}` names the demanded mode: {message}"
            );
            assert!(
                !message.contains("invalid PostgreSQL connection string"),
                "`{url}` reports the TLS demand instead of a syntax error: {message}"
            );
        }
    }

    #[test]
    fn the_driver_probe_answers_for_the_mode_it_models() {
        // The probe is what keeps the parse-failure fallback honest: the mode this
        // build really supports is reported by the driver itself, while a spelling
        // it does not support is never silently treated as one.
        assert!(driver_models_sslmode("require"));
        assert!(!driver_models_sslmode("maybe"));
    }
}
