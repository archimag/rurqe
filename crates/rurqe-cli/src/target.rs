//! The database a connection string addresses.
//!
//! Commands open either a live database or a disposable scratch state from a
//! URL, so the rules that decide which engine a string addresses live in one
//! place. A URL form is classified by its scheme as the standard `url` parser
//! reports it — `postgres` / `postgresql` address PostgreSQL and `sqlite`
//! addresses SQLite — and a value that parser rejects is classified by the
//! libpq keyword/value shape, `:memory:`, or its file extension, so a
//! connection string that names its engine is never reclassified by the shape
//! of the database name it ends with. A value that names no engine this build
//! connects to is rejected with a [`TargetError`] instead of being guessed to
//! be PostgreSQL.

use std::fmt;

/// The database engine a connection string targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DatabaseDialect {
    /// PostgreSQL, selected by `postgres://` / `postgresql://` URLs and libpq
    /// keyword/value strings.
    Postgres,
    /// SQLite, selected by `sqlite:` URLs or local `.sqlite` / `.sqlite3` / `.db` files.
    Sqlite,
}

impl fmt::Display for DatabaseDialect {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            DatabaseDialect::Postgres => "PostgreSQL",
            DatabaseDialect::Sqlite => "SQLite",
        })
    }
}

/// The database a connection string addresses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DatabaseTarget {
    /// A PostgreSQL connection string, which the driver parses itself.
    Postgres,
    /// The in-memory SQLite database of `:memory:` or of a bare `sqlite:` target.
    SqliteMemory,
    /// A file-backed SQLite database, with the `sqlite:` scheme already removed.
    SqliteFile(String),
}

/// The reason a connection string names no database this build connects to.
///
/// The error names the scheme where the string spells one and never carries the
/// string itself: a value that reaches this error may hold credentials, so the
/// caller renders it redacted instead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TargetError {
    /// The URL scheme names no engine this build connects to.
    UnrecognisedScheme {
        /// The scheme as the standard `url` parser reports it.
        scheme: String,
    },
    /// The scheme-less value names neither a libpq keyword/value string,
    /// `:memory:`, nor a SQLite file path.
    Unrecognised,
}

impl DatabaseTarget {
    /// Classifies the connection string `url`, rejecting a value that names no
    /// engine this build connects to.
    ///
    /// A URL form is classified by its scheme as the standard `url` parser
    /// reports it: `postgres` and `postgresql` address PostgreSQL, `sqlite`
    /// addresses SQLite through the location [`sqlite_url_path`] reads, and any
    /// other scheme is rejected with its name instead of being guessed to be
    /// PostgreSQL. A value that parser rejects is no URL this build can classify
    /// by scheme: it is a libpq keyword/value string, `:memory:`, a SQLite file
    /// path — or a malformed URL of a supported engine, whose own driver has to
    /// report the defect, which is why the scheme prefixes keep routing to their
    /// engine ahead of the rejection.
    pub fn parse(url: &str) -> Result<Self, TargetError> {
        let url = url.trim();

        if let Ok(parsed) = url::Url::parse(url) {
            return match parsed.scheme() {
                "postgres" | "postgresql" => Ok(DatabaseTarget::Postgres),
                "sqlite" => Ok(sqlite_target(sqlite_url_path(url).unwrap_or_default())),
                scheme => Err(TargetError::UnrecognisedScheme {
                    scheme: scheme.to_owned(),
                }),
            };
        }

        if names_postgres(url) {
            return Ok(DatabaseTarget::Postgres);
        }

        if let Some(path) = sqlite_url_path(url) {
            return Ok(sqlite_target(path));
        }

        if url == ":memory:" {
            return Ok(DatabaseTarget::SqliteMemory);
        }

        // A libpq keyword/value string carries a `key=value` pair; the driver
        // validates the keywords once the target opens.
        if url.contains('=') {
            return Ok(DatabaseTarget::Postgres);
        }

        sqlite_file_target(url).ok_or(TargetError::Unrecognised)
    }

    /// The engine this target addresses.
    pub fn dialect(&self) -> DatabaseDialect {
        match self {
            DatabaseTarget::Postgres => DatabaseDialect::Postgres,
            DatabaseTarget::SqliteMemory | DatabaseTarget::SqliteFile(_) => DatabaseDialect::Sqlite,
        }
    }
}

/// Detects the target [`DatabaseDialect`] from a connection string.
///
/// Absent URLs default to [`DatabaseDialect::Postgres`], preserving the
/// historical behavior for callers that rely on the default, and a value
/// [`DatabaseTarget::parse`] rejects keeps that default: this helper only
/// picks a flavour for a value the caller supplied on its own, while every
/// connection passes through the strict gate of [`DatabaseTarget::parse`] and
/// is rejected there instead of being misread.
pub fn detect_dialect(database_url: Option<&str>) -> DatabaseDialect {
    match database_url {
        Some(url) => DatabaseTarget::parse(url)
            .map_or(DatabaseDialect::Postgres, |target| target.dialect()),
        None => DatabaseDialect::Postgres,
    }
}

/// Whether `url` opens with a PostgreSQL URL scheme.
///
/// Reached only for a string the standard `url` parser rejects, so a malformed
/// PostgreSQL URL is still routed to the driver that reports the defect.
fn names_postgres(url: &str) -> bool {
    ["postgres", "postgresql"]
        .into_iter()
        .any(|scheme| strip_scheme(url, scheme).is_some())
}

/// Strips a case-insensitive `scheme:` prefix, returning what follows it.
///
/// A value shorter than `scheme`, or one whose bytes would split a character,
/// names no scheme and yields `None`.
fn strip_scheme<'a>(url: &'a str, scheme: &str) -> Option<&'a str> {
    let head = url.get(..scheme.len())?;

    if !head.eq_ignore_ascii_case(scheme) {
        return None;
    }

    url.get(scheme.len()..)?.strip_prefix(':')
}

/// The file a `sqlite:` connection string opens.
///
/// The location is derived exactly once, here, because the classifier that opens
/// the database and the diagnostics that report it have to name the same file.
/// The returned path carries neither the scheme, the `//` of a URL, the userinfo
/// of its authority, nor its query string, and `None` reports a value that does
/// not spell the `sqlite:` scheme at all.
pub(crate) fn sqlite_url_path(url: &str) -> Option<&str> {
    let location = strip_scheme(url.trim(), "sqlite")?;

    Some(sqlite_path(location))
}

/// Classifies the path a `sqlite:` connection string opens.
fn sqlite_target(path: &str) -> DatabaseTarget {
    if path.is_empty() || path == ":memory:" {
        DatabaseTarget::SqliteMemory
    } else {
        DatabaseTarget::SqliteFile(path.to_owned())
    }
}

/// Reduces what follows a `sqlite:` scheme to the file it opens.
///
/// A scheme-qualified spelling is a URL, so the location ends where its query
/// string or fragment begins — `sqlite:app.db?mode=rwc` opens `app.db` — and the
/// userinfo of its authority is dropped: `sqlite://user:pass@host/db` opens
/// `host/db`, the very file `sqlite://host/db` opens, so a credential never
/// becomes a directory of the opened path and no diagnostic has to render one.
/// SQLite interprets URI parameters only for names that begin with `file:`, so
/// keeping the query in the location would open a file whose literal name
/// contains it.
fn sqlite_path(location: &str) -> &str {
    let location = location_without_query(location);

    match location.strip_prefix("//") {
        Some(authority_and_path) => after_userinfo(authority_and_path),
        None => location,
    }
}

/// Drops the userinfo a URL authority carries.
///
/// The authority is the segment before the first `/`, and everything up to its
/// last `@` is userinfo, which names no path. A location without an `@` there is
/// returned unchanged, and the leading `/` of `sqlite:///var/lib/app.db` leaves
/// the authority empty, so a file name that genuinely carries `@` keeps it.
fn after_userinfo(authority_and_path: &str) -> &str {
    let authority_end = authority_and_path
        .find('/')
        .unwrap_or(authority_and_path.len());
    let authority = authority_and_path
        .get(..authority_end)
        .unwrap_or_default();

    match authority.rfind('@') {
        // `@` is an ASCII character, so the index after it always falls on a
        // scalar boundary.
        Some(at) => authority_and_path
            .get(at + 1..)
            .unwrap_or(authority_and_path),
        None => authority_and_path,
    }
}

/// Truncates `path` where the query string or fragment of its URL begins.
///
/// The delimiter is an ASCII character, so the byte index `find` reports always
/// falls on a character boundary and the slice never splits a scalar value.
fn location_without_query(path: &str) -> &str {
    match path.find(['?', '#']) {
        Some(end) => path.get(..end).unwrap_or(path),
        None => path,
    }
}

/// The file extensions SQLite projects use for their databases.
const SQLITE_FILE_EXTENSIONS: [&str; 3] = ["db", "sqlite", "sqlite3"];

/// Classifies a scheme-less value as a SQLite database file.
///
/// The extension is read with [`std::path::Path::extension`], so the value is a
/// file path exactly the way the platform spells one, and a value whose final
/// component carries no extension — a bare name such as `app`, or a dotfile —
/// classifies as nothing instead of being guessed to be PostgreSQL. A libpq
/// keyword/value string never reaches here: it carries a `key=value` pair, and
/// the database name it ends with must not reclassify the connection string.
fn sqlite_file_target(url: &str) -> Option<DatabaseTarget> {
    let extension = std::path::Path::new(url).extension()?.to_str()?;

    SQLITE_FILE_EXTENSIONS
        .into_iter()
        .any(|known| extension.eq_ignore_ascii_case(known))
        .then(|| DatabaseTarget::SqliteFile(url.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_postgres_url_stays_postgres_whatever_its_database_is_called() {
        for url in [
            "postgres://db.internal:5432/app",
            "postgres://db.internal/app.db",
            "postgresql://db.internal:5432/app.sqlite3",
        ] {
            assert_eq!(
                DatabaseTarget::parse(url),
                Ok(DatabaseTarget::Postgres),
                "{url}"
            );
            assert_eq!(
                detect_dialect(Some(url)),
                DatabaseDialect::Postgres,
                "{url}"
            );
        }
    }

    #[test]
    fn a_keyword_connection_string_is_never_taken_for_a_file() {
        for url in ["host=db.internal dbname=app.db", "dbname=app.db user=app"] {
            assert_eq!(
                DatabaseTarget::parse(url),
                Ok(DatabaseTarget::Postgres),
                "{url}"
            );
        }
    }

    #[test]
    fn a_sqlite_url_names_the_file_it_opens() {
        assert_eq!(
            DatabaseTarget::parse("sqlite:///var/lib/app.db"),
            Ok(DatabaseTarget::SqliteFile("/var/lib/app.db".to_owned()))
        );
        assert_eq!(
            DatabaseTarget::parse("sqlite:data/app.sqlite3"),
            Ok(DatabaseTarget::SqliteFile("data/app.sqlite3".to_owned()))
        );
    }

    #[test]
    fn a_sqlite_url_opens_the_location_its_query_does_not_belong_to() {
        for (url, expected) in [
            ("sqlite:app.db?mode=rwc", "app.db"),
            ("sqlite:/tmp/x.db?a=1#f", "/tmp/x.db"),
            ("sqlite:///var/lib/app.db?cache=shared", "/var/lib/app.db"),
            ("sqlite://host/app.db#fragment", "host/app.db"),
        ] {
            assert_eq!(
                DatabaseTarget::parse(url),
                Ok(DatabaseTarget::SqliteFile(expected.to_owned())),
                "{url}"
            );
        }
    }

    #[test]
    fn a_userinfo_bearing_url_opens_the_file_that_carries_no_credential() {
        for url in [
            "sqlite://user:pass@host/db",
            "sqlite://user@host/db",
            "sqlite://user:pass@host/db?mode=rwc",
        ] {
            assert_eq!(
                DatabaseTarget::parse(url),
                Ok(DatabaseTarget::SqliteFile("host/db".to_owned())),
                "{url}"
            );
        }
    }

    #[test]
    fn a_file_name_carrying_an_at_sign_keeps_it() {
        for url in ["sqlite:/var/lib/app@2.db", "sqlite:///var/lib/app@2.db"] {
            assert_eq!(
                DatabaseTarget::parse(url),
                Ok(DatabaseTarget::SqliteFile("/var/lib/app@2.db".to_owned())),
                "{url}"
            );
        }
    }

    #[test]
    fn the_in_memory_spelling_survives_a_query_string() {
        assert_eq!(
            DatabaseTarget::parse("sqlite::memory:?cache=shared"),
            Ok(DatabaseTarget::SqliteMemory)
        );
    }

    #[test]
    fn a_scheme_less_file_name_keeps_the_literal_question_mark_it_spells() {
        assert_eq!(
            DatabaseTarget::parse("data/app?.db"),
            Ok(DatabaseTarget::SqliteFile("data/app?.db".to_owned()))
        );
    }

    #[test]
    fn an_in_memory_target_needs_no_file() {
        for url in [":memory:", "sqlite::memory:", "sqlite:", "sqlite://"] {
            assert_eq!(
                DatabaseTarget::parse(url),
                Ok(DatabaseTarget::SqliteMemory),
                "{url}"
            );
        }
    }

    #[test]
    fn a_bare_path_is_recognised_by_its_extension() {
        for url in ["app.db", "data/app.sqlite", "data/app.sqlite3"] {
            assert_eq!(
                DatabaseTarget::parse(url),
                Ok(DatabaseTarget::SqliteFile(url.to_owned())),
                "{url}"
            );
            assert_eq!(detect_dialect(Some(url)), DatabaseDialect::Sqlite, "{url}");
        }
    }

    #[test]
    fn an_unrecognised_value_is_rejected_instead_of_defaulting_to_postgres() {
        assert_eq!(DatabaseTarget::parse("app"), Err(TargetError::Unrecognised));
        assert_eq!(DatabaseTarget::parse(""), Err(TargetError::Unrecognised));
        assert_eq!(detect_dialect(None), DatabaseDialect::Postgres);
        // The defaulting helper keeps its Postgres default for a value the
        // connection gate rejects: picking a flavour opens nothing, and every
        // connection still passes through the gate.
        assert_eq!(detect_dialect(Some("app")), DatabaseDialect::Postgres);
    }

    #[test]
    fn an_unknown_url_scheme_is_rejected_with_its_name() {
        for (url, scheme) in [
            ("mysql://app@db.internal:3306/app", "mysql"),
            ("MYSQL://app@db.internal/app", "mysql"),
            ("app:memory", "app"),
        ] {
            assert_eq!(
                DatabaseTarget::parse(url),
                Err(TargetError::UnrecognisedScheme {
                    scheme: scheme.to_owned()
                }),
                "{url}"
            );
        }
    }

    #[test]
    fn the_dialect_renders_its_canonical_name() {
        assert_eq!(DatabaseDialect::Sqlite.to_string(), "SQLite");
        assert_eq!(DatabaseDialect::Postgres.to_string(), "PostgreSQL");
        assert_eq!(
            DatabaseTarget::SqliteFile("app.db".to_owned()).dialect(),
            DatabaseDialect::Sqlite
        );
    }
}
