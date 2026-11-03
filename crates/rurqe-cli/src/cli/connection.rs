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

use crate::target::{DatabaseTarget, TargetError};

use super::error::{CliCause, CliError, TransportRefusal};
use super::redact::{host_name, redact_database_url};

/// The environment variable a command falls back to when its flag names no target.
const DATABASE_URL_VARIABLE: &str = "DATABASE_URL";

/// Resolves the target database URL from the flag or the `DATABASE_URL` environment variable.
///
/// The flag wins over the environment, and the environment is read only when the
/// invocation named no target: a variable the command never consults cannot refuse
/// it. A variable that is present but not valid UTF-8 travels as its own failure
/// from [`environment_database_url`] instead of being folded into the "nothing
/// supplied" answer, so the diagnostic never claims that no target was configured
/// for a target that is configured.
pub(super) fn resolve_database_url(supplied: Option<String>) -> Result<String, CliError> {
    let resolved = match supplied {
        Some(url) => Some(url),
        None => environment_database_url()?,
    };

    resolved
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            CliError::message(format!(
                "no database URL provided: pass `--database-url <url>` or set the \
                 {DATABASE_URL_VARIABLE} environment variable"
            ))
        })
}

/// Reads `DATABASE_URL`, keeping an absent variable apart from an undecodable one.
///
/// `std::env::var` reports `VarError::NotUnicode` for a variable the platform can
/// represent but Unicode cannot, so reducing the read to `Option` — the shape
/// `.ok()` produces — would state that nothing was configured while the variable is
/// set. The crate keeps the same distinction on the argument surface, where
/// `arguments_to_utf8` rejects a non-UTF-8 argument by position instead of
/// panicking during collection; this is the environment half of it. The value is
/// never echoed and never rewritten into replacement characters, because a
/// connection string may carry credentials.
pub(super) fn environment_database_url() -> Result<Option<String>, CliError> {
    classify_database_url(std::env::var(DATABASE_URL_VARIABLE))
}

/// Classifies what reading `DATABASE_URL` answered.
///
/// The classification is separated from the read so both answers of the
/// environment — decodable, and present but undecodable — are covered without a
/// test mutating the process environment the rest of the suite shares.
fn classify_database_url(
    read: Result<String, std::env::VarError>,
) -> Result<Option<String>, CliError> {
    match read {
        Ok(value) => Ok(Some(value)),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(std::env::VarError::NotUnicode(_)) => Err(CliError::message(format!(
            "the `{DATABASE_URL_VARIABLE}` environment variable is not valid UTF-8: set it to \
             UTF-8 text, since a database URL carrying raw bytes is rejected rather than \
             rewritten"
        ))),
    }
}

/// A live database connection opened from a URL or filesystem path.
pub(crate) enum Connection {
    /// SQLite database handle.
    Sqlite(rusqlite::Connection),
    /// PostgreSQL client.
    ///
    /// The client is boxed because it carries the session protocol buffers and
    /// is far larger than the SQLite handle, so keeping it inline would size
    /// every `Connection` — and every match arm over one — by the PostgreSQL
    /// driver.
    Postgres(Box<postgres::Client>),
}

/// Opens a connection to the database identified by `url`, detecting its engine.
///
/// This is the single entry point for scheme stripping, `:memory:` handling and
/// connection error wording, so the CLI commands and catalog introspection can
/// never drift apart. A value that names no engine this build connects to is
/// rejected here, at the gate, so no unrecognised target is ever handed to a
/// driver that could only misread it.
pub(crate) fn connect(url: &str) -> Result<Connection, CliError> {
    match DatabaseTarget::parse(url) {
        Ok(DatabaseTarget::Postgres) => Ok(Connection::Postgres(Box::new(connect_postgres(url)?))),
        Ok(DatabaseTarget::SqliteMemory) => open_sqlite(None, url).map(Connection::Sqlite),
        Ok(DatabaseTarget::SqliteFile(path)) => {
            open_sqlite(Some(path.as_str()), url).map(Connection::Sqlite)
        }
        Err(error) => Err(unrecognised_target(error, url)),
    }
}

/// Reports a connection string that names no engine this build connects to.
///
/// The message names the scheme where the string spells one, renders the string
/// redacted, and states both engines' addressing forms once, so an unrecognised
/// target is rejected at the connection gate with one wording instead of being
/// handed to a driver that can only misread it.
fn unrecognised_target(error: TargetError, url: &str) -> CliError {
    let scheme = match error {
        TargetError::UnrecognisedScheme { scheme } => Some(scheme),
        TargetError::Unrecognised => None,
    };

    CliError::with_cause(CliCause::Transport(TransportRefusal::UnrecognisedTarget {
        scheme,
        url: redact_database_url(url),
    }))
}

/// Opens a SQLite connection, in memory when the target names no file.
fn open_sqlite(path: Option<&str>, url: &str) -> Result<rusqlite::Connection, CliError> {
    let conn = match path {
        Some(path) => rusqlite::Connection::open(path),
        None => rusqlite::Connection::open_in_memory(),
    };

    conn.map_err(|e| {
        CliError::message(format!(
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
    let config =
        url.parse::<postgres::Config>()
            .map_err(|error| match unmodelled_tls_demand(url) {
                Some(spelling) => tls_unavailable(spelling, url),
                None => CliError::message(format!(
                    "invalid PostgreSQL connection string `{}`: {error}",
                    redact_database_url(url)
                )),
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

        CliError::message(format!(
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

    Err(CliError::with_cause(CliCause::Transport(
        TransportRefusal::UnacknowledgedCleartext {
            host,
            ssl_mode: ssl_mode_name(ssl_mode),
            url: redact_database_url(url),
        },
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
    CliError::with_cause(CliCause::Transport(TransportRefusal::TlsUnavailable {
        ssl_mode: ssl_mode.to_owned(),
        url: redact_database_url(url),
    }))
}

/// The `sslmode` modes libpq defines that demand TLS.
///
/// One source names every mode and its libpq spelling: a mode the linked driver
/// does not model never reaches the typed `SslMode`, which is exactly why it
/// has to be recognised here — the connection string fails parsing otherwise,
/// and the operator would be told the string is malformed instead of being
/// told that the encryption it demands is unavailable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TlsDemandingMode {
    /// `sslmode=require`.
    Require,
    /// `sslmode=verify-ca`.
    VerifyCa,
    /// `sslmode=verify-full`.
    VerifyFull,
}

impl TlsDemandingMode {
    /// Every mode libpq defines that demands TLS.
    const ALL: [Self; 3] = [Self::Require, Self::VerifyCa, Self::VerifyFull];

    /// The spelling of the mode in a libpq connection string.
    fn libpq_spelling(self) -> &'static str {
        match self {
            Self::Require => "require",
            Self::VerifyCa => "verify-ca",
            Self::VerifyFull => "verify-full",
        }
    }
}

/// The name libpq gives the transport-mode parameter of a connection string.
const SSLMODE_PARAMETER: &str = "sslmode";

/// The TLS-demanding `sslmode` spelling a connection string asks for while the
/// driver's parser does not model it.
///
/// This is the fallback the parse failure in [`connect_postgres`] consults: a spelling
/// the driver does not know makes the whole string unparseable, so the typed
/// `SslMode` never exists and the policy has nothing to classify.
/// [`sslmode_parameter`] reads the value out of either documented libpq form instead of
/// reconstructing one from the other, and [`driver_models_sslmode`] keeps the fallback
/// honest: a spelling the driver understands is reported by the policy, never here, so
/// an unrelated defect in a connection string is not blamed on TLS.
fn unmodelled_tls_demand(url: &str) -> Option<&'static str> {
    let spelling = sslmode_parameter(url)?;

    TlsDemandingMode::ALL
        .into_iter()
        .find(|mode| mode.libpq_spelling() == spelling.as_str())
        .filter(|mode| !driver_models_sslmode(mode.libpq_spelling()))
        .map(TlsDemandingMode::libpq_spelling)
}

/// Reads the `sslmode` parameter out of either documented libpq connection-string form.
///
/// libpq accepts two spellings of the same connection and both are understood here,
/// without either one being reconstructed from the other:
///
/// * the URL form — `postgres://host/db?sslmode=require` — where the value is read from
///   the query component of the parsed [`url::Url`] through
///   [`url::form_urlencoded::parse`], so percent-encoding and repeated keys follow that
///   parser's documented rules;
/// * the keyword/value form — `host=db sslmode=require` — where the string is split on
///   ASCII whitespace and every token at its first `=` into a key and a value, and the
///   value has its surrounding single or double quotes removed.
///
/// The key comparison is case-insensitive, because libpq parameter names are not
/// case-sensitive (`SSLMODE=` and `SslMode=` mean the same as `sslmode=`), and the
/// result is `None` only when the string names the parameter in neither form.
fn sslmode_parameter(url: &str) -> Option<String> {
    if let Ok(parsed) = url::Url::parse(url) {
        if is_postgres_scheme(parsed.scheme()) {
            return url::form_urlencoded::parse(parsed.query().unwrap_or_default().as_bytes())
                .find(|(key, _)| key.eq_ignore_ascii_case(SSLMODE_PARAMETER))
                .map(|(_, value)| unquoted(value.as_ref()));
        }
    }

    keyword_parameter(url, SSLMODE_PARAMETER)
}

/// Whether a URL scheme names a PostgreSQL connection string.
///
/// Only the two schemes libpq registers take the query-component path; every other
/// string — the keyword/value form included, which carries no scheme at all — is read
/// as a keyword list.
fn is_postgres_scheme(scheme: &str) -> bool {
    matches!(scheme, "postgres" | "postgresql")
}

/// Reads a parameter out of the libpq keyword/value form of a connection string.
///
/// The string is split on ASCII whitespace and every token at its first `=`, so a value
/// that itself contains `=` keeps the remainder while a token holding no `=` is skipped.
/// The key is compared case-insensitively, so the keyword form yields the same result as
/// the URL form of the same connection.
fn keyword_parameter(url: &str, parameter: &str) -> Option<String> {
    url.split_ascii_whitespace().find_map(|token| {
        let (key, value) = token.split_once('=')?;

        key.eq_ignore_ascii_case(parameter).then(|| unquoted(value))
    })
}

/// Removes the single or double quotes libpq allows around a keyword-form value.
fn unquoted(value: &str) -> String {
    value
        .trim_matches(|character: char| character == '\'' || character == '"')
        .to_owned()
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
        assert!(error.cause().is_none());
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

        let Some(CliCause::Transport(refusal)) = error.cause() else {
            panic!("a TLS demand is a transport cause");
        };
        match refusal {
            TransportRefusal::TlsUnavailable { ssl_mode, .. } => {
                assert_eq!(ssl_mode.as_str(), "require");
            }
            other => panic!("the refusal names the demanded mode, got {other:?}"),
        }
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

        let Some(CliCause::Transport(refusal)) = error.cause() else {
            panic!("an implicit downgrade is a transport cause");
        };
        match refusal {
            TransportRefusal::UnacknowledgedCleartext { host, ssl_mode, .. } => {
                assert_eq!(host.as_str(), "db.internal");
                assert_eq!(ssl_mode.as_str(), "prefer");
            }
            other => panic!("the refusal names the remote host, got {other:?}"),
        }
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

        let Some(CliCause::Transport(refusal)) = error.cause() else {
            panic!("a remote host on the list is a transport cause");
        };
        match refusal {
            TransportRefusal::UnacknowledgedCleartext { host, .. } => {
                assert_eq!(host.as_str(), "db.internal");
            }
            other => panic!("the refusal names the remote host, got {other:?}"),
        }
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

            let Some(CliCause::Transport(refusal)) = error.cause() else {
                panic!("`{url}` reports its TLS demand as a transport cause");
            };
            match refusal {
                TransportRefusal::TlsUnavailable { ssl_mode, .. } => {
                    assert!(
                        ssl_mode.starts_with("verify-"),
                        "`{url}` names the demanded mode: {ssl_mode}"
                    );
                }
                other => panic!("`{url}` names the demanded mode, got {other:?}"),
            }
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

    #[test]
    fn the_sslmode_parameter_reads_both_libpq_forms_alike() {
        // The same connection written both ways has to yield the same value, so the
        // fallback never depends on which form the operator happened to write.
        for (url, expected) in [
            ("postgres://app@db.internal/app?sslmode=require", "require"),
            (
                "host=db.internal user=app dbname=app sslmode=require",
                "require",
            ),
            (
                "postgresql://app@db.internal/app?sslmode=verify-full",
                "verify-full",
            ),
            (
                "host=db.internal user=app sslmode='verify-full'",
                "verify-full",
            ),
        ] {
            assert_eq!(sslmode_parameter(url).as_deref(), Some(expected), "`{url}`");
        }
    }

    #[test]
    fn the_sslmode_parameter_matches_the_key_case_insensitively() {
        for url in [
            "postgres://app@db.internal/app?SSLMODE=require",
            "postgres://app@db.internal/app?SslMode=require",
            "host=db.internal user=app SSLMODE=require",
            "host=db.internal user=app SslMode=require",
        ] {
            assert_eq!(
                sslmode_parameter(url).as_deref(),
                Some("require"),
                "`{url}`"
            );
        }
    }

    #[test]
    fn the_sslmode_parameter_reports_an_absent_parameter_as_missing() {
        for url in [
            "postgres://app@db.internal/app",
            "postgres://app@db.internal/app?connect_timeout=5",
            "postgresql://app@db.internal/app?user=app",
            "host=db.internal user=app dbname=app",
            "host=db.internal user=app",
            "db.internal",
        ] {
            assert_eq!(sslmode_parameter(url), None, "`{url}`");
        }
    }

    #[test]
    fn a_mode_the_policy_accepts_is_never_reported_by_the_fallback() {
        // The fallback only speaks about a TLS demand the driver could not model, so a
        // mode the policy accepts — or a string naming no mode at all — stays silent and
        // an unrelated parse failure keeps its own wording.
        for url in [
            "postgres://app@db.internal/app?sslmode=disable",
            "postgres://app@db.internal/app?sslmode=prefer",
            "host=db.internal user=app sslmode=disable",
            "host=db.internal user=app sslmode=maybe",
            "host=db.internal user=app",
        ] {
            assert_eq!(unmodelled_tls_demand(url), None, "`{url}`");
        }
    }

    #[test]
    fn the_tls_demanding_modes_render_their_libpq_spellings() {
        assert_eq!(TlsDemandingMode::Require.libpq_spelling(), "require");
        assert_eq!(TlsDemandingMode::VerifyCa.libpq_spelling(), "verify-ca");
        assert_eq!(TlsDemandingMode::VerifyFull.libpq_spelling(), "verify-full");
        assert_eq!(
            TlsDemandingMode::ALL.map(TlsDemandingMode::libpq_spelling),
            ["require", "verify-ca", "verify-full"]
        );
    }

    /// A variable the process does not carry names no target at all, so the
    /// resolution stays free to report that nothing was supplied.
    #[test]
    fn an_absent_variable_names_no_target() {
        let read = classify_database_url(Err(std::env::VarError::NotPresent));
        let absent = read.expect("an absent variable is not a failure");

        assert_eq!(absent, None);
    }

    /// A variable the platform can represent but Unicode cannot is a failure
    /// of its own: the resolution never claims that no target was configured,
    /// which is exactly what reducing the read to `Option` used to fabricate.
    #[test]
    fn an_undecodable_variable_is_reported_instead_of_treated_as_unset() {
        let undecodable = std::env::VarError::NotUnicode(std::ffi::OsString::new());
        let read = classify_database_url(Err(undecodable));
        let rejected = read.expect_err("the variable is set but unreadable");
        let message = rejected.to_string();

        assert!(message.contains("DATABASE_URL"), "got {message}");
        assert!(message.contains("not valid UTF-8"), "got {message}");
        assert!(
            !message.contains("no database URL provided"),
            "the diagnostic never claims the variable is unset: {message}"
        );
    }

    /// A variable that decodes is the target it names, verbatim.
    #[test]
    fn a_decodable_variable_carries_the_target_it_names() {
        let url = "postgres://app@db.internal:5432/app";
        let read = classify_database_url(Ok(url.to_owned()));
        let target = read.expect("a decodable variable is not a failure");

        assert_eq!(target, Some(url.to_owned()));
    }

    #[test]
    fn an_unrecognised_target_is_rejected_at_the_connection_gate() {
        let bare = connect("app").err().expect("a bare name opens no database");
        assert!(
            bare.to_string().contains("names no database engine"),
            "got {bare}"
        );

        let schemed = connect("mysql://app@db.internal:3306/app")
            .err()
            .expect("a scheme this build connects to neither opens no database");
        let message = schemed.to_string();
        assert!(
            message.contains("names the URL scheme `mysql`"),
            "got {message}"
        );
        assert!(
            message.contains("PostgreSQL is addressed by"),
            "got {message}"
        );

        let Some(CliCause::Transport(bare_refusal)) = bare.cause() else {
            panic!("a bare value that names no engine is a transport cause");
        };
        match bare_refusal {
            TransportRefusal::UnrecognisedTarget { scheme: None, .. } => {}
            other => panic!("a bare value spells no scheme, got {other:?}"),
        }

        let Some(CliCause::Transport(schemed_refusal)) = schemed.cause() else {
            panic!("an unrecognised scheme is a transport cause");
        };
        match schemed_refusal {
            TransportRefusal::UnrecognisedTarget { scheme: Some(scheme), .. } => {
                assert_eq!(scheme.as_str(), "mysql");
            }
            other => panic!("the refusal names the spelled scheme, got {other:?}"),
        }
    }
}
