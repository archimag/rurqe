//! The disposable scratch state one side of the equation is materialised in.

use rurqe_migrate::catalog::SchemaCatalog;

use crate::cli::redact_database_url;
use crate::config::SchemaSelection;
use crate::introspect::introspect_sqlite;
use crate::target::DatabaseTarget;

use super::error::StageError;
use super::postgres::PostgresSandbox;

/// One labelled SQL fragment applied to a scratch state.
pub(super) struct Script<'a> {
    /// The file name of a stage script or the script name of a migration.
    pub(super) label: &'a str,
    /// The SQL the fragment contributes.
    pub(super) sql: &'a str,
    /// Whether the fragment is replayed inside a transaction.
    pub(super) transactional: bool,
}

/// The disposable database state one side of the equation is materialised in.
///
/// The variants are dispatched statically, so a verification pays for exactly
/// the engine it targets and no trait object sits on the hot path.
pub(super) enum SandboxKind {
    /// An in-memory SQLite database, so the filesystem stays untouched.
    ///
    /// Every side opens its own memory database, so nothing is left to undo.
    SqliteMemory(rusqlite::Connection),
    /// A file-backed SQLite database, deleted once its side ends.
    ///
    /// Deleting the file is the only reset that also clears the database header
    /// (`PRAGMA user_version`, `PRAGMA application_id`) and the connection
    /// settings a replayed script can change.
    SqliteFile {
        /// The live connection of the current side.
        connection: rusqlite::Connection,
        /// The file the connection was opened in, removed again on discard.
        path: String,
    },
    /// An ephemeral PostgreSQL database, dropped once the verification ends.
    ///
    /// The sandbox carries live clients and a database name, so it is boxed to
    /// keep every value of this enum small regardless of the targeted engine.
    Postgres(Box<PostgresSandbox>),
}

impl SandboxKind {
    /// Opens the scratch state addressed by `scratch_url`, introspecting `schemas`.
    ///
    /// The whole selection is described at once — the primary schema plus every
    /// secondary schema the project declares — so one side materialises the summed
    /// catalog of the project. A SQLite scratch holds a single unnamed catalog, so
    /// the selection is ignored there and reaches only the PostgreSQL side,
    /// mirroring
    /// [`introspect_database_selection`](crate::introspect::introspect_database_selection).
    ///
    /// A file-backed SQLite scratch has to be empty, so a misconfigured
    /// `--scratch-url` never replays schema DDL on top of somebody's data.
    pub(super) fn open(scratch_url: &str, schemas: &SchemaSelection) -> Result<Self, StageError> {
        match DatabaseTarget::parse(scratch_url) {
            Ok(DatabaseTarget::Postgres) => PostgresSandbox::connect(scratch_url, schemas)
                .map(Box::new)
                .map(SandboxKind::Postgres)
                .map_err(|message| StageError::Sandbox { message }),
            Ok(DatabaseTarget::SqliteMemory) => open_sqlite_memory(scratch_url),
            Ok(DatabaseTarget::SqliteFile(path)) => open_sqlite_file(&path, scratch_url),
            // A scratch URL naming no engine this build connects to is refused
            // redacted: the string may carry the credentials of the engine it
            // addresses, so the failure must not echo them.
            Err(_) => Err(StageError::Sandbox {
                message: format!(
                    "the scratch URL `{}` names no database engine this build connects to; \
                     point `--scratch-url` at a `postgres://` URL or at a SQLite target",
                    redact_database_url(scratch_url)
                ),
            }),
        }
    }

    /// Throws the scratch state away once a side of the equation was introspected.
    ///
    /// The in-memory SQLite database closes with its connection, the file-backed
    /// one is deleted together with its write-ahead log, and the ephemeral
    /// PostgreSQL database is dropped with its sandbox. A script that committed
    /// half of its statements therefore leaves nothing behind for the next side.
    pub(super) fn discard(self) -> Result<(), StageError> {
        match self {
            // Closing the connection disposes of the whole in-memory database.
            SandboxKind::SqliteMemory(_) => Ok(()),
            SandboxKind::SqliteFile { connection, path } => {
                drop(connection);
                remove_scratch_file(&path)
            }
            // The sandbox drops the ephemeral database together with its clients.
            SandboxKind::Postgres(_) => Ok(()),
        }
    }

    /// Applies one SQL script to the scratch state.
    ///
    /// A transactional script runs inside a transaction, so its statements commit
    /// together or not at all; a non-transactional one commits every statement on
    /// its own, exactly as the migration runner replays it in production.
    pub(super) fn apply(&mut self, sql: &str, transactional: bool) -> Result<(), String> {
        match self {
            SandboxKind::SqliteMemory(connection) | SandboxKind::SqliteFile { connection, .. } => {
                apply_sqlite(connection, sql, transactional)
            }
            SandboxKind::Postgres(sandbox) => {
                if transactional {
                    sandbox.apply_transactional(sql)
                } else {
                    sandbox.apply_non_transactional(sql)
                }
            }
        }
    }

    /// Introspects the catalog currently materialised by the scratch state.
    pub(super) fn catalog(&mut self) -> Result<SchemaCatalog, String> {
        match self {
            SandboxKind::SqliteMemory(connection) | SandboxKind::SqliteFile { connection, .. } => {
                introspect_sqlite(connection).map_err(|error| error.to_string())
            }
            SandboxKind::Postgres(sandbox) => sandbox.catalog(),
        }
    }

    /// The vacuous-selection warning the materialised catalog produced, if any.
    ///
    /// A SQLite scratch holds a single unnamed catalog, so no selection can
    /// describe nothing and the notice is always absent, mirroring
    /// `crate::introspect::report_vacuous_selection`. The PostgreSQL side delegates
    /// to the sandbox, so a scratch database whose selected schemas hold no
    /// relation while it keeps user relations elsewhere reports the schemas that
    /// do and the `--schema` remedy instead of certifying equivalence.
    pub(super) fn vacuous_schema_notice(&mut self) -> Result<Option<String>, String> {
        match self {
            // A SQLite scratch has no schema layer, so the selection always
            // describes the catalog the replay produced.
            SandboxKind::SqliteMemory(_) | SandboxKind::SqliteFile { .. } => Ok(None),
            SandboxKind::Postgres(sandbox) => sandbox.vacuous_schema_notice(),
        }
    }
}

/// The user objects a SQLite scratch database currently holds.
const SQLITE_SCRATCH_OBJECTS: &str = "SELECT name FROM sqlite_master \
     WHERE type IN ('table', 'view') AND name NOT LIKE 'sqlite_%' \
     ORDER BY name";

/// Applies one SQL script to a SQLite scratch database under the declared mode.
fn apply_sqlite(
    connection: &mut rusqlite::Connection,
    sql: &str,
    transactional: bool,
) -> Result<(), String> {
    if !transactional {
        return connection
            .execute_batch(sql)
            .map_err(|error| error.to_string());
    }

    let transaction = connection
        .transaction()
        .map_err(|error| error.to_string())?;

    transaction
        .execute_batch(sql)
        .map_err(|error| error.to_string())?;

    transaction.commit().map_err(|error| error.to_string())
}

/// Opens the in-memory SQLite scratch database a bare `sqlite:` target names.
///
/// `:memory:`, an empty value, and a bare `sqlite:` prefix the target has
/// already reduced to nothing all end up here, so the replay never touches the
/// filesystem.
fn open_sqlite_memory(scratch_url: &str) -> Result<SandboxKind, StageError> {
    rusqlite::Connection::open_in_memory()
        .map(SandboxKind::SqliteMemory)
        .map_err(|error| StageError::Sandbox {
            message: format!(
                "failed to open the SQLite scratch database `{}`: {error}",
                redact_database_url(scratch_url)
            ),
        })
}

/// Opens the file-backed SQLite scratch database the target names.
///
/// The file is required to be empty, and it is deleted once the side that opened
/// it ends, so a file that outlives its verification can never feed one side of
/// the equation with the state of another.
fn open_sqlite_file(path: &str, scratch_url: &str) -> Result<SandboxKind, StageError> {
    let connection = rusqlite::Connection::open(path).map_err(|error| StageError::Sandbox {
        message: format!(
            "failed to open the SQLite scratch database `{}`: {error}",
            redact_database_url(scratch_url)
        ),
    })?;

    ensure_scratch_is_empty(&connection, scratch_url)?;

    Ok(SandboxKind::SqliteFile {
        connection,
        path: path.to_owned(),
    })
}

/// Refuses a scratch database that already holds user objects.
fn ensure_scratch_is_empty(
    connection: &rusqlite::Connection,
    scratch_url: &str,
) -> Result<(), StageError> {
    let objects = scratch_objects(connection)?;

    if objects.is_empty() {
        return Ok(());
    }

    let names: Vec<&str> = objects.iter().map(String::as_str).collect();

    Err(StageError::Sandbox {
        message: format!(
            "the SQLite scratch database `{}` is not empty (it holds {}); a previous \
             verification may have left those objects behind when it failed, so point \
             `--scratch-url` at a fresh file or at `:memory:`",
            redact_database_url(scratch_url),
            names.join(", ")
        ),
    })
}

/// The names of the user objects of a SQLite scratch database.
fn scratch_objects(connection: &rusqlite::Connection) -> Result<Vec<String>, StageError> {
    let mut statement = connection
        .prepare(SQLITE_SCRATCH_OBJECTS)
        .map_err(|error| StageError::Sandbox {
            message: format!("failed to inspect the SQLite scratch database: {error}"),
        })?;

    let objects = statement
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|error| StageError::Sandbox {
            message: format!("failed to inspect the SQLite scratch database: {error}"),
        })?
        .collect::<Result<Vec<String>, _>>()
        .map_err(|error| StageError::Sandbox {
            message: format!("failed to read the SQLite scratch database: {error}"),
        })?;

    Ok(objects)
}

/// Deletes the file a scratch database was materialised in.
///
/// The write-ahead log and the shared-memory index are removed together with the
/// database itself, because a sidecar that outlives its database would otherwise
/// reach the file a later side creates under the same name.
fn remove_scratch_file(path: &str) -> Result<(), StageError> {
    for suffix in ["", "-wal", "-shm"] {
        let target = format!("{path}{suffix}");

        match std::fs::remove_file(&target) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(StageError::Sandbox {
                    message: format!(
                        "failed to delete the SQLite scratch database `{target}`: {error}"
                    ),
                })
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// SQL whose first statement succeeds and whose second one always fails.
    const HALF_BROKEN: &str = "CREATE TABLE kept (id INTEGER PRIMARY KEY);\n\
                               CREATE TABLE kept (id INTEGER PRIMARY KEY);";

    #[test]
    fn a_failed_transactional_script_rolls_back_its_statements() {
        // A SQLite scratch holds one unnamed catalog, so the schema name is
        // accepted and ignored instead of selecting anything.
        let mut sandbox = SandboxKind::open(":memory:", &SchemaSelection::single("reporting"))
            .expect("an in-memory scratch opens");

        let error = sandbox
            .apply(HALF_BROKEN, true)
            .expect_err("the script cannot create the same table twice");
        assert!(error.contains("kept"), "unexpected message: {error}");

        let catalog = sandbox.catalog().expect("the scratch is introspectable");
        assert!(
            catalog.tables.is_empty(),
            "a transactional replay leaves nothing behind: {:?}",
            catalog.tables
        );
    }

    #[test]
    fn a_failed_non_transactional_script_keeps_what_it_committed() {
        let mut sandbox = SandboxKind::open(":memory:", &SchemaSelection::single("reporting"))
            .expect("an in-memory scratch opens");

        sandbox
            .apply(HALF_BROKEN, false)
            .expect_err("the script cannot create the same table twice");

        let catalog = sandbox.catalog().expect("the scratch is introspectable");
        let names: Vec<&str> = catalog
            .tables
            .iter()
            .map(|table| table.name.as_str())
            .collect();

        assert_eq!(
            names,
            vec!["kept"],
            "a non-transactional replay commits statement by statement"
        );
    }
}
