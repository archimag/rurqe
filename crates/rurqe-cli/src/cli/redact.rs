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
//! * a SQLite target is parsed by `url::Url`, which drops the userinfo, the query
//!   string, and the fragment before the location is re-rendered;
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

/// Replaces every credential a connection string carries.
///
/// The returned diagnostic is reconstructed from typed fields, so the original
/// string is never echoed. A PostgreSQL string the driver refuses — an unknown
/// parameter, a malformed URL, an unsupported `sslmode` — yields the fixed
/// placeholder above, and a bare SQLite file name is echoed verbatim because it
/// holds no credential.
pub fn redact_database_url(url: &str) -> String {
    match DatabaseTarget::parse(url) {
        DatabaseTarget::Postgres => redact_postgres(url),
        DatabaseTarget::SqliteMemory => ":memory:".to_owned(),
        DatabaseTarget::SqliteFile(path) => redact_sqlite(url, &path),
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
fn host_name(host: &postgres::config::Host) -> String {
    match host {
        postgres::config::Host::Tcp(name) => name.clone(),
        _ => "unix-socket".to_owned(),
    }
}

/// Rebinds a SQLite target to the location its URL parser reduced it to.
///
/// The parser drops the userinfo, the query string, and the fragment, so a
/// secret spelled as a SQLite query parameter cannot survive. A scheme-less
/// value — a bare `app.db` path — is not a URL at all, and the target classifier
/// only admits such a value when it names a plain file, so the path is echoed as
/// it stands.
fn redact_sqlite(url: &str, path: &str) -> String {
    match url::Url::parse(url) {
        Ok(parsed) => {
            let scheme = parsed.scheme();
            let location = parsed.path();
            format!("{scheme}:{location}")
        }
        Err(_) => path.to_owned(),
    }
}
