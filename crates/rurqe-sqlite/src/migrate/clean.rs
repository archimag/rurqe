//! The destructive reset of a SQLite target database.
//!
//! SQLite registers every object of a database file in `sqlite_master`, so the
//! catalog itself is the manifest of what a clean erases: one pass over that table
//! enumerates the user tables — the `__rurqe_schema_history` table among them — the
//! views, and the triggers, and every enumerated object is dropped by name.
//!
//! The drops need no topological order. The adapter never turns foreign key
//! enforcement on (the driver default), so a table with children drops cleanly, and
//! the triggers of a dropped table go with it: dropping one of them again by name is
//! a no-op of `DROP TRIGGER IF EXISTS`.
//!
//! The reset runs inside one step of the surrounding run, so the drops are applied
//! together or not at all: an object that refuses to drop discards the ones already
//! issued instead of leaving the target half erased.

use rurqe_migrate::error::Error;
use rurqe_migrate::runner::{MigrationBackend, MigrationTransaction};

use crate::introspect::escape_sqlite_identifier;

use super::history::backend_error;
use super::SqliteMigrationBackend;

/// The catalog statement enumerating the objects a clean drops.
///
/// The order is deterministic — the kind first, the name second — so the manifest an
/// operator is shown and the sequence the reset really drops are the same one.
/// Internal objects carry the reserved `sqlite_` prefix, and an index is never
/// listed on its own: dropping its table drops it.
const CLEAN_CATALOG: &str = r#"
SELECT type, name
FROM sqlite_master
WHERE type IN ('table', 'view', 'trigger')
  AND name NOT LIKE 'sqlite_%'
ORDER BY type, name;
"#;

/// The kinds of object a clean drops.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CleanKind {
    /// A user table, the history table among them.
    Table,
    /// A view.
    View,
    /// A trigger.
    Trigger,
}

impl CleanKind {
    /// Reads the kind off the catalog value.
    ///
    /// The enumeration asks for these three kinds only, so a value this reading
    /// does not recognise is reported instead of being dropped under a guessed
    /// keyword.
    fn from_catalog(kind: &str) -> Option<Self> {
        match kind {
            "table" => Some(Self::Table),
            "view" => Some(Self::View),
            "trigger" => Some(Self::Trigger),
            _ => None,
        }
    }

    /// The keyword the drop statement of the kind is spelled with.
    fn keyword(self) -> &'static str {
        match self {
            Self::Table => "TABLE",
            Self::View => "VIEW",
            Self::Trigger => "TRIGGER",
        }
    }
}

/// One enumerated object of the target database.
#[derive(Debug, Clone, PartialEq, Eq)]
struct CleanTarget {
    /// The kind the object is dropped as.
    kind: CleanKind,
    /// The name the object is registered under.
    name: String,
}

impl CleanTarget {
    /// The statement dropping the object, its name quoted as an identifier.
    fn drop_sql(&self) -> String {
        format!(
            "DROP {} IF EXISTS \"{}\"",
            self.kind.keyword(),
            escape_sqlite_identifier(&self.name)
        )
    }
}

/// Lists the objects a clean of `connection` drops.
pub(super) fn clean_manifest(connection: &::rusqlite::Connection) -> Result<Vec<String>, Error> {
    Ok(clean_targets(connection)?
        .into_iter()
        .map(|target| target.name)
        .collect())
}

/// Drops every object a clean of `backend` enumerates.
pub(super) fn clean(backend: &mut SqliteMigrationBackend) -> Result<Vec<String>, Error> {
    backend.reject_poisoned()?;
    let targets = clean_targets(&backend.conn)?;

    let mut step = backend.begin()?;
    for target in &targets {
        step.execute(&target.drop_sql())?;
    }
    step.commit()?;

    Ok(targets.into_iter().map(|target| target.name).collect())
}

/// Reads the objects a clean drops off the catalog of `connection`.
fn clean_targets(connection: &::rusqlite::Connection) -> Result<Vec<CleanTarget>, Error> {
    let mut statement = connection.prepare(CLEAN_CATALOG).map_err(backend_error)?;
    let rows = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(backend_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(backend_error)?;

    let mut targets = Vec::with_capacity(rows.len());
    for (catalog_kind, name) in rows {
        let kind = CleanKind::from_catalog(&catalog_kind).ok_or_else(|| {
            Error::Backend(format!(
                "the SQLite catalog reported `{name}` as a `{catalog_kind}`, which the clean does \
                 not drop"
            ))
        })?;
        targets.push(CleanTarget { kind, name });
    }

    Ok(targets)
}
