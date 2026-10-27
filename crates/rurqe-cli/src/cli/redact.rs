//! Masking of the credentials a connection string carries.
//!
//! Connection strings routinely embed secrets and the diagnostics derived from
//! them end up in CI logs, so a failure reports a connection string rebuilt from
//! the typed fields a real parser produced instead of the string the caller
//! passed in. Hosts, ports, the database name, and the engine stay visible —
//! they identify the target that failed — while the user name, the password, and
//! every other parameter are structurally excluded:
//!
//! * a PostgreSQL target is parsed by the driver's own `postgres::Config`, the
//!   parser `connect` uses, and re-rendered from its user, host, port, and
//!   database-name fields;
//! * a SQLite target has its location derived by `crate::target::sqlite_url_path`,
//!   the very function the classifier that opens the database calls, so the
//!   diagnostic names the file the command works on and the location carries no
//!   userinfo, query string, or fragment to rebuild in the first place;
//! * a connection string neither parser accepts is replaced by a fixed
//!   placeholder that embeds no byte of the input.
//!
//! Because nothing of the original text is copied into the answer, a secret
//! spelled in a way this module never anticipated — a percent-encoded parameter
//! name, a `-c password=…` option, a `service` indirection, a backslash-escaped
//! libpq value — cannot survive, and no hand-written grammar has to track the
//! spellings of libpq, URL userinfo, or query parameters.
//!
//! A bare SQLite file name (`app.db`) is the only verbatim passthrough: the
//! database-target classifier admits a scheme-less value only when it names a
//! plain file, so such a path carries no credential grammar at all.

use crate::target::DatabaseTarget;

/// The diagnostic reported for a PostgreSQL connection string no parser accepts.
///
/// The placeholder embeds no byte of the input: a string this crate cannot parse
/// is exactly the string whose secrets the redactor cannot locate, so nothing of
/// it is echoed.
const UNPARSEABLE_POSTGRES: &str = "<unparseable PostgreSQL connection string>";

/// The diagnostic reported for a connection string that names no engine this
/// build connects to.
///
/// The target classifier rejects such a value instead of guessing an engine,
/// and the string may carry the credentials of the engine it addresses, so the
/// redaction replaces it wholesale: no byte of the input reaches a log.
const UNRECOGNISED_TARGET: &str = "<unrecognised database URL>";

/// Replaces every credential a connection string carries.
///
/// The returned diagnostic is reconstructed from typed fields, so the original
/// string is never echoed. A PostgreSQL string the driver refuses — an unknown
/// parameter, a malformed URL, an unsupported `sslmode` — yields the fixed
/// placeholder above, a connection string the target classifier rejects yields
/// the unrecognised-target placeholder, and a bare SQLite file name is echoed
/// verbatim because it holds no credential.
pub fn redact_database_url(url: &str) -> String {
    match DatabaseTarget::parse(url) {
        Ok(DatabaseTarget::Postgres) => redact_postgres(url),
        Ok(DatabaseTarget::SqliteMemory) => ":memory:".to_owned(),
        Ok(DatabaseTarget::SqliteFile(path)) => redact_sqlite(url, &path),
        Err(_) => UNRECOGNISED_TARGET.to_owned(),
    }
}

/// Rebuilds a PostgreSQL connection string from the fields its driver parsed.
fn redact_postgres(url: &str) -> String {
    match url.parse::<postgres::Config>() {
        Ok(config) => render_postgres(&config),
        Err(_) => UNPARSEABLE_POSTGRES.to_owned(),
    }
}

/// The port libpq connects to when a connection string names none.
///
/// The driver reports the effective port of every target, so a host the caller
/// never spelled a port for arrives carrying this value. It identifies nothing
/// about the target the caller chose, so it stays out of the diagnostic while
/// every port the caller did spell remains visible.
const DEFAULT_POSTGRES_PORT: u16 = 5432;

/// Renders `postgres://[***@]host[:port][,host[:port]][/dbname]`.
///
/// Every field comes from the parsed configuration, so a user name is replaced
/// by `***` as soon as one is set and no parameter value of the original string —
/// the password, `sslpassword`, `options`, `service` — is reproduced. A host
/// configured without a port renders without one, whether the driver reports the
/// default port for it or reports none at all, so the answer never depends on how
/// the parser fills that gap.
fn render_postgres(config: &postgres::Config) -> String {
    let mut rendered = String::from("postgres://");

    if config.get_user().is_some() {
        rendered.push_str("***@");
    }

    let ports = config.get_ports();
    for (index, host) in config.get_hosts().iter().enumerate() {
        if index > 0 {
            rendered.push(',');
        }

        rendered.push_str(&host_name(host));

        let port = ports
            .get(index)
            .copied()
            .filter(|port| *port != DEFAULT_POSTGRES_PORT);

        if let Some(port) = port {
            rendered.push(':');
            rendered.push_str(&port.to_string());
        }
    }

    if let Some(dbname) = config.get_dbname() {
        rendered.push('/');
        rendered.push_str(dbname);
    }

    rendered
}

/// The host a parsed host specification names.
///
/// A TCP specification renders as the host name libpq was given; any other
/// specification — the Unix-socket path of a libpq `host=/var/run/…` value, or a
/// variant a later driver release adds — renders as a fixed marker, so the
/// answer never reproduces text the input carried.
pub(super) fn host_name(host: &postgres::config::Host) -> String {
    match host {
        postgres::config::Host::Tcp(name) => name.clone(),
        _ => "unix-socket".to_owned(),
    }
}

/// Rebinds a SQLite target to the location the shared derivation reduced it to.
///
/// The derivation `crate::target::sqlite_url_path` performs is the one the
/// classifier that opens the database performs, so the diagnostic names the very
/// file the command works on instead of a form of it re-derived by a second
/// parser. It drops the userinfo, the query string, and the fragment, so a secret
/// spelled as a SQLite user name, password, or URI parameter cannot survive into
/// the answer. A scheme-less value — a bare `app.db` path — spells no `sqlite:`
/// scheme at all and is not a URL, and the target classifier only admits such a
/// value when it names a plain file, so that path is echoed as it stands.
fn redact_sqlite(url: &str, path: &str) -> String {
    match crate::target::sqlite_url_path(url) {
        Some(location) => format!("sqlite:{location}"),
        None => path.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_redacted_sqlite_url_names_the_file_that_is_opened() {
        for url in [
            "sqlite:app.db?mode=rwc",
            "sqlite:/tmp/x.db?a=1#f",
            "sqlite:///var/lib/app.db?cache=shared",
            "sqlite://db.internal/app.sqlite3?mode=ro",
            "sqlite://user:pass@db.internal/app.sqlite3",
            "sqlite://user@db.internal/app.db?mode=rwc",
            "app.sqlite",
        ] {
            let opened = match DatabaseTarget::parse(url) {
                Ok(DatabaseTarget::SqliteFile(path)) => path,
                other => panic!("{url} did not name a file: {other:?}"),
            };

            let redacted = redact_database_url(url);
            let reported = redacted.strip_prefix("sqlite:").unwrap_or(&redacted);

            assert_eq!(reported, opened, "{url}");
        }
    }

    #[test]
    fn a_userinfo_bearing_sqlite_url_reports_the_file_without_the_credential() {
        for (url, opened) in [
            ("sqlite://user:pass@host/db", "host/db"),
            ("sqlite://user@host/db", "host/db"),
            ("sqlite://user:pass@host/app.db?mode=rwc", "host/app.db"),
        ] {
            assert_eq!(
                DatabaseTarget::parse(url),
                Ok(DatabaseTarget::SqliteFile(opened.to_owned())),
                "{url}"
            );

            let redacted = redact_database_url(url);

            assert_eq!(redacted, format!("sqlite:{opened}"), "{url}");

            for credential in ["user", "pass"] {
                assert!(
                    !redacted.contains(credential),
                    "{url} leaked {credential} into {redacted}"
                );
            }
        }
    }

    #[test]
    fn a_target_the_classifier_rejects_renders_as_a_placeholder() {
        for url in ["mysql://app:s3cret@db.internal:3306/app", "app", ""] {
            let redacted = redact_database_url(url);

            assert_eq!(redacted, UNRECOGNISED_TARGET, "{url}");
            assert!(!redacted.contains("s3cret"), "{url}");
        }
    }

    #[test]
    fn an_in_memory_target_renders_as_the_canonical_spelling() {
        for url in [":memory:", "sqlite::memory:", "sqlite:", "sqlite://"] {
            assert_eq!(redact_database_url(url), ":memory:", "{url}");
        }
    }
}
