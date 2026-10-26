//! The database a connection string addresses.
//!
//! Commands open either a live database or a disposable scratch state from a
//! URL, so the rules that decide which engine a string addresses live in one
//! place. A scheme (`postgres://`, `postgresql://`, `sqlite:`) settles the
//! question on its own, and only a scheme-less value — a bare SQLite path —
//! falls back to the file extensions: a connection string that names its engine
//! must never be reclassified by the shape of the database name it ends with.

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

impl DatabaseTarget {
    /// Classifies the connection string `url`.
    pub fn parse(url: &str) -> Self {
        let url = url.trim();

        if names_postgres(url) {
            return DatabaseTarget::Postgres;
        }

        if let Some(path) = sqlite_url_path(url) {
            return sqlite_target(path);
        }

        if url == ":memory:" {
            return DatabaseTarget::SqliteMemory;
        }

        if names_sqlite_file(url) {
            return DatabaseTarget::SqliteFile(url.to_owned());
        }

        DatabaseTarget::Postgres
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
/// historical behavior for callers that rely on the default.
pub fn detect_dialect(database_url: Option<&str>) -> DatabaseDialect {
    match database_url {
        Some(url) => DatabaseTarget::parse(url).dialect(),
        None => DatabaseDialect::Postgres,
    }
}

/// Whether `url` opens with a PostgreSQL URL scheme.
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

/// Whether a scheme-less value names a SQLite database file.
///
/// Only values that look like a file path are classified: a libpq keyword/value
/// string such as `host=db dbname=app.db` carries an `=`, and the database name
/// it ends with must not reclassify the connection string.
fn names_sqlite_file(url: &str) -> bool {
    // The file extensions SQLite projects use for their databases.
    const EXTENSIONS: [&str; 3] = [".db", ".sqlite", ".sqlite3"];

    if url.is_empty() || url.contains('=') {
        return false;
    }

    let lowered = url.to_ascii_lowercase();
    EXTENSIONS
        .into_iter()
        .any(|extension| lowered.ends_with(extension))
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
                DatabaseTarget::Postgres,
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
                DatabaseTarget::Postgres,
                "{url}"
            );
        }
    }

    #[test]
    fn a_sqlite_url_names_the_file_it_opens() {
        assert_eq!(
            DatabaseTarget::parse("sqlite:///var/lib/app.db"),
            DatabaseTarget::SqliteFile("/var/lib/app.db".to_owned())
        );
        assert_eq!(
            DatabaseTarget::parse("sqlite:data/app.sqlite3"),
            DatabaseTarget::SqliteFile("data/app.sqlite3".to_owned())
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
                DatabaseTarget::SqliteFile(expected.to_owned()),
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
                DatabaseTarget::SqliteFile("host/db".to_owned()),
                "{url}"
            );
        }
    }

    #[test]
    fn a_file_name_carrying_an_at_sign_keeps_it() {
        for url in ["sqlite:/var/lib/app@2.db", "sqlite:///var/lib/app@2.db"] {
            assert_eq!(
                DatabaseTarget::parse(url),
                DatabaseTarget::SqliteFile("/var/lib/app@2.db".to_owned()),
                "{url}"
            );
        }
    }

    #[test]
    fn the_in_memory_spelling_survives_a_query_string() {
        assert_eq!(
            DatabaseTarget::parse("sqlite::memory:?cache=shared"),
            DatabaseTarget::SqliteMemory
        );
    }

    #[test]
    fn a_scheme_less_file_name_keeps_the_literal_question_mark_it_spells() {
        assert_eq!(
            DatabaseTarget::parse("data/app?.db"),
            DatabaseTarget::SqliteFile("data/app?.db".to_owned())
        );
    }

    #[test]
    fn an_in_memory_target_needs_no_file() {
        for url in [":memory:", "sqlite::memory:", "sqlite:", "sqlite://"] {
            assert_eq!(
                DatabaseTarget::parse(url),
                DatabaseTarget::SqliteMemory,
                "{url}"
            );
        }
    }

    #[test]
    fn a_bare_path_is_recognised_by_its_extension() {
        for url in ["app.db", "data/app.sqlite", "data/app.sqlite3"] {
            assert_eq!(
                DatabaseTarget::parse(url),
                DatabaseTarget::SqliteFile(url.to_owned()),
                "{url}"
            );
            assert_eq!(detect_dialect(Some(url)), DatabaseDialect::Sqlite, "{url}");
        }
    }

    #[test]
    fn an_unrecognised_value_falls_back_to_postgres() {
        assert_eq!(DatabaseTarget::parse("app"), DatabaseTarget::Postgres);
        assert_eq!(DatabaseTarget::parse(""), DatabaseTarget::Postgres);
        assert_eq!(detect_dialect(None), DatabaseDialect::Postgres);
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
