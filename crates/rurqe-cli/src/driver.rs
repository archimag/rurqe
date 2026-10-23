//! Migration lifecycle driver adapters.
//!
//! [`SqliteMigrationBackend`] and [`PostgresMigrationBackend`] translate the
//! engine-neutral [`MigrationBackend`] contract into driver calls. Every step
//! opens a native driver transaction, so a migration script and its
//! `__rurqe_schema_history` row are always applied or discarded together.
//!
//! Both adapters also report the canonical digest of the catalog they hold, so a
//! cold start anchors the very digest `schema.lock` and `checkpoint.toml` carry
//! whatever engine executed the declarative stage.

use rurqe_migrate::catalog::SchemaCatalog;
use rurqe_migrate::checksum::Checksum;
use rurqe_migrate::error::Error;
use rurqe_migrate::history::{
    create_schema_history_table_sql, SchemaHistoryEntry, SCHEMA_HISTORY_TABLE,
};
use rurqe_migrate::migration::MigrationKind;
use rurqe_migrate::runner::{MigrationBackend, MigrationTransaction};
use rurqe_migrate::version::MigrationVersion;

use crate::introspect::{introspect_postgres, introspect_postgres_schema, introspect_sqlite};

/// Number of columns written by [`SchemaHistoryEntry`].
const HISTORY_COLUMN_COUNT: usize = 9;

/// Advisory lock key guarding a PostgreSQL migration run.
const MIGRATION_LOCK_ID: i64 = 0x5255_5251_455f_4d49;

/// A raw `__rurqe_schema_history` row before decoding into domain types.
struct RawHistoryRow {
    installed_rank: i64,
    version: Option<String>,
    description: String,
    kind: String,
    script: String,
    checksum: Option<String>,
    installed_on: String,
    execution_time_ms: i64,
    success: bool,
}

impl RawHistoryRow {
    fn decode(self) -> Result<SchemaHistoryEntry, Error> {
        Ok(SchemaHistoryEntry {
            installed_rank: self.installed_rank,
            version: match self.version {
                Some(raw) => Some(raw.parse::<MigrationVersion>()?),
                None => None,
            },
            description: self.description,
            kind: decode_kind(&self.kind)?,
            script: self.script,
            checksum: match self.checksum {
                Some(hex) => Some(Checksum::from_hex(&hex)?),
                None => None,
            },
            installed_on: self.installed_on,
            execution_time_ms: self.execution_time_ms,
            success: self.success,
        })
    }
}

fn decode_kind(raw: &str) -> Result<MigrationKind, Error> {
    [
        MigrationKind::Versioned,
        MigrationKind::Repeatable,
        MigrationKind::Baseline,
    ]
    .into_iter()
    .find(|kind| kind.as_str() == raw)
    .ok_or_else(|| {
        Error::Backend(format!(
            "unknown migration kind `{raw}` recorded in {SCHEMA_HISTORY_TABLE}"
        ))
    })
}

fn select_history_sql() -> String {
    format!(
        "SELECT installed_rank, version, description, \"type\", script, checksum, installed_on, \
         execution_time, success FROM {SCHEMA_HISTORY_TABLE} ORDER BY installed_rank"
    )
}

fn insert_entry_sql(sigil: char) -> String {
    format!(
        "INSERT INTO {SCHEMA_HISTORY_TABLE} (installed_rank, version, description, \"type\", \
         script, checksum, installed_on, execution_time, success) VALUES ({})",
        placeholders(sigil, HISTORY_COLUMN_COUNT)
    )
}

fn delete_entry_sql(sigil: char) -> String {
    format!("DELETE FROM {SCHEMA_HISTORY_TABLE} WHERE installed_rank = {sigil}1")
}

fn update_checksum_sql(sigil: char) -> String {
    format!("UPDATE {SCHEMA_HISTORY_TABLE} SET checksum = {sigil}1 WHERE script = {sigil}2")
}

fn placeholders(sigil: char, count: usize) -> String {
    (1..=count)
        .map(|index| format!("{sigil}{index}"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn backend_error(err: impl std::fmt::Display) -> Error {
    Error::Backend(err.to_string())
}

/// [`MigrationBackend`] over a `rusqlite` connection.
pub struct SqliteMigrationBackend {
    conn: rusqlite::Connection,
}

impl SqliteMigrationBackend {
    /// Wraps an open SQLite connection.
    pub fn new(conn: rusqlite::Connection) -> Self {
        Self { conn }
    }

    /// Introspects the schema catalog of the wrapped connection.
    ///
    /// The catalog is read through the very introspection the rest of the
    /// toolchain uses, so the history table stays out of it and the reported
    /// types follow the SQLite affinity rules.
    pub fn introspect_catalog(&self) -> Result<SchemaCatalog, Error> {
        introspect_sqlite(&self.conn).map_err(backend_error)
    }
}

impl MigrationBackend for SqliteMigrationBackend {
    type Transaction<'a> = SqliteMigrationTransaction<'a>;

    fn ensure_schema_history_table(&mut self) -> Result<(), Error> {
        self.conn
            .execute_batch(create_schema_history_table_sql())
            .map_err(backend_error)
    }

    fn read_history(&mut self) -> Result<Vec<SchemaHistoryEntry>, Error> {
        let mut statement = self
            .conn
            .prepare(&select_history_sql())
            .map_err(backend_error)?;

        let rows = statement
            .query_map([], |row| {
                Ok(RawHistoryRow {
                    installed_rank: row.get(0)?,
                    version: row.get(1)?,
                    description: row.get(2)?,
                    kind: row.get(3)?,
                    script: row.get(4)?,
                    checksum: row.get(5)?,
                    installed_on: row.get(6)?,
                    execution_time_ms: row.get(7)?,
                    success: row.get::<_, i64>(8)? != 0,
                })
            })
            .map_err(backend_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(backend_error)?;

        rows.into_iter().map(RawHistoryRow::decode).collect()
    }

    fn catalog_checksum(&mut self) -> Result<Option<Checksum>, Error> {
        self.introspect_catalog()
            .map(|catalog| Some(catalog.checksum()))
    }

    fn begin(&mut self) -> Result<Self::Transaction<'_>, Error> {
        let tx = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(backend_error)?;
        Ok(SqliteMigrationTransaction { tx: Some(tx) })
    }
}

/// Transactional boundary of a [`SqliteMigrationBackend`] step.
pub struct SqliteMigrationTransaction<'a> {
    tx: Option<rusqlite::Transaction<'a>>,
}

impl<'a> SqliteMigrationTransaction<'a> {
    fn transaction(&mut self) -> Result<&mut rusqlite::Transaction<'a>, Error> {
        self.tx.as_mut().ok_or_else(|| {
            Error::Backend("sqlite migration transaction already finished".to_owned())
        })
    }

    /// Introspects the schema catalog the open transaction sees.
    ///
    /// The catalog is read over the connection of the transaction, so a step
    /// observes the changes it has already applied without committing them.
    pub fn introspect_catalog(&mut self) -> Result<SchemaCatalog, Error> {
        let tx = self.transaction()?;
        introspect_sqlite(&*tx).map_err(backend_error)
    }

    fn run(&mut self, sql: &str, params: impl rusqlite::Params) -> Result<(), Error> {
        self.transaction()?
            .execute(sql, params)
            .map(|_| ())
            .map_err(backend_error)
    }
}

impl MigrationTransaction for SqliteMigrationTransaction<'_> {
    fn execute(&mut self, sql: &str) -> Result<(), Error> {
        self.transaction()?.execute_batch(sql).map_err(backend_error)
    }

    fn catalog_checksum(&mut self) -> Result<Option<Checksum>, Error> {
        self.introspect_catalog()
            .map(|catalog| Some(catalog.checksum()))
    }

    fn record_entry(&mut self, entry: &SchemaHistoryEntry) -> Result<(), Error> {
        let sql = insert_entry_sql('?');
        let version = entry.version.as_ref().map(ToString::to_string);
        let checksum = entry.checksum.map(|checksum| checksum.to_hex());

        self.run(
            &sql,
            rusqlite::params![
                entry.installed_rank,
                version,
                entry.description,
                entry.kind.as_str(),
                entry.script,
                checksum,
                entry.installed_on,
                entry.execution_time_ms,
                entry.success,
            ],
        )
    }

    fn delete_entry(&mut self, installed_rank: i64) -> Result<(), Error> {
        let sql = delete_entry_sql('?');
        self.run(&sql, rusqlite::params![installed_rank])
    }

    fn update_checksum(&mut self, script: &str, checksum: &Checksum) -> Result<(), Error> {
        let sql = update_checksum_sql('?');
        self.run(&sql, rusqlite::params![checksum.to_hex(), script])
    }

    fn commit(mut self) -> Result<(), Error> {
        let tx = self.tx.take().ok_or_else(|| {
            Error::Backend("sqlite migration transaction already finished".to_owned())
        })?;
        tx.commit().map_err(backend_error)
    }

    fn rollback(mut self) -> Result<(), Error> {
        let tx = self.tx.take().ok_or_else(|| {
            Error::Backend("sqlite migration transaction already finished".to_owned())
        })?;
        tx.rollback().map_err(backend_error)
    }
}

impl Drop for SqliteMigrationTransaction<'_> {
    fn drop(&mut self) {
        if let Some(tx) = self.tx.take() {
            if let Err(err) = tx.rollback() {
                tracing::warn!(error = %err, "failed to rollback sqlite migration transaction");
            }
        }
    }
}

/// [`MigrationBackend`] over a `postgres` client.
pub struct PostgresMigrationBackend {
    client: postgres::Client,
    lock_held: bool,
}

impl PostgresMigrationBackend {
    /// Wraps an open PostgreSQL client.
    pub fn new(client: postgres::Client) -> Self {
        Self {
            client,
            lock_held: false,
        }
    }

    /// Introspects the schema catalog of the `public` schema.
    pub fn introspect_catalog(&mut self) -> Result<SchemaCatalog, Error> {
        introspect_postgres(&mut self.client).map_err(backend_error)
    }
}

impl MigrationBackend for PostgresMigrationBackend {
    type Transaction<'a> = PostgresMigrationTransaction<'a>;

    fn ensure_schema_history_table(&mut self) -> Result<(), Error> {
        self.client
            .batch_execute(create_schema_history_table_sql())
            .map_err(backend_error)
    }

    fn read_history(&mut self) -> Result<Vec<SchemaHistoryEntry>, Error> {
        let rows = self
            .client
            .query(&select_history_sql(), &[])
            .map_err(backend_error)?;

        let mut history = Vec::with_capacity(rows.len());
        for row in &rows {
            let raw = RawHistoryRow {
                installed_rank: row.try_get(0).map_err(backend_error)?,
                version: row.try_get(1).map_err(backend_error)?,
                description: row.try_get(2).map_err(backend_error)?,
                kind: row.try_get(3).map_err(backend_error)?,
                script: row.try_get(4).map_err(backend_error)?,
                checksum: row.try_get(5).map_err(backend_error)?,
                installed_on: row.try_get(6).map_err(backend_error)?,
                execution_time_ms: row.try_get(7).map_err(backend_error)?,
                success: row.try_get(8).map_err(backend_error)?,
            };
            history.push(raw.decode()?);
        }

        Ok(history)
    }

    fn catalog_checksum(&mut self) -> Result<Option<Checksum>, Error> {
        self.introspect_catalog()
            .map(|catalog| Some(catalog.checksum()))
    }

    fn begin(&mut self) -> Result<Self::Transaction<'_>, Error> {
        let tx = self.client.transaction().map_err(backend_error)?;
        Ok(PostgresMigrationTransaction { tx: Some(tx) })
    }

    fn acquire_lock(&mut self) -> Result<(), Error> {
        self.client
            .query("SELECT pg_advisory_lock($1)", &[&MIGRATION_LOCK_ID])
            .map(|_| ())
            .map_err(backend_error)?;
        self.lock_held = true;
        Ok(())
    }

    fn release_lock(&mut self) -> Result<(), Error> {
        // The flag is cleared before the query runs: a connection that already
        // dropped releases the lock on the server side by itself, and retrying
        // the unlock on the same closed connection from `Drop` cannot succeed.
        self.lock_held = false;
        self.client
            .query("SELECT pg_advisory_unlock($1)", &[&MIGRATION_LOCK_ID])
            .map(|_| ())
            .map_err(backend_error)
    }
}

impl Drop for PostgresMigrationBackend {
    fn drop(&mut self) {
        if !self.lock_held {
            return;
        }
        if let Err(err) = self.release_lock() {
            tracing::warn!(error = %err, "failed to release migration advisory lock");
        }
    }
}

/// Transactional boundary of a [`PostgresMigrationBackend`] step.
pub struct PostgresMigrationTransaction<'a> {
    tx: Option<postgres::Transaction<'a>>,
}

impl<'a> PostgresMigrationTransaction<'a> {
    fn transaction(&mut self) -> Result<&mut postgres::Transaction<'a>, Error> {
        self.tx.as_mut().ok_or_else(|| {
            Error::Backend("postgres migration transaction already finished".to_owned())
        })
    }

    /// Introspects the `public` schema as the open transaction sees it.
    ///
    /// The catalog statements run over the transaction itself, so a step reads
    /// the changes it has applied so far and nobody else observes them until the
    /// transaction commits.
    pub fn introspect_catalog(&mut self) -> Result<SchemaCatalog, Error> {
        introspect_postgres_schema(self.transaction()?, "public").map_err(backend_error)
    }
}

impl MigrationTransaction for PostgresMigrationTransaction<'_> {
    fn execute(&mut self, sql: &str) -> Result<(), Error> {
        self.transaction()?.batch_execute(sql).map_err(backend_error)
    }

    fn catalog_checksum(&mut self) -> Result<Option<Checksum>, Error> {
        self.introspect_catalog()
            .map(|catalog| Some(catalog.checksum()))
    }

    fn record_entry(&mut self, entry: &SchemaHistoryEntry) -> Result<(), Error> {
        let sql = insert_entry_sql('$');
        let version = entry.version.as_ref().map(ToString::to_string);
        let checksum = entry.checksum.map(|checksum| checksum.to_hex());
        let kind = entry.kind.as_str();

        let params: [&(dyn postgres::types::ToSql + Sync); HISTORY_COLUMN_COUNT] = [
            &entry.installed_rank,
            &version,
            &entry.description,
            &kind,
            &entry.script,
            &checksum,
            &entry.installed_on,
            &entry.execution_time_ms,
            &entry.success,
        ];

        self.transaction()?
            .execute(&sql, &params)
            .map(|_| ())
            .map_err(backend_error)
    }

    fn delete_entry(&mut self, installed_rank: i64) -> Result<(), Error> {
        let sql = delete_entry_sql('$');
        self.transaction()?
            .execute(&sql, &[&installed_rank])
            .map(|_| ())
            .map_err(backend_error)
    }

    fn update_checksum(&mut self, script: &str, checksum: &Checksum) -> Result<(), Error> {
        let sql = update_checksum_sql('$');
        let hex = checksum.to_hex();
        self.transaction()?
            .execute(&sql, &[&hex, &script])
            .map(|_| ())
            .map_err(backend_error)
    }

    fn commit(mut self) -> Result<(), Error> {
        let tx = self.tx.take().ok_or_else(|| {
            Error::Backend("postgres migration transaction already finished".to_owned())
        })?;
        tx.commit().map_err(backend_error)
    }

    fn rollback(mut self) -> Result<(), Error> {
        let tx = self.tx.take().ok_or_else(|| {
            Error::Backend("postgres migration transaction already finished".to_owned())
        })?;
        tx.rollback().map_err(backend_error)
    }
}

impl Drop for PostgresMigrationTransaction<'_> {
    fn drop(&mut self) {
        if let Some(tx) = self.tx.take() {
            if let Err(err) = tx.rollback() {
                tracing::warn!(error = %err, "failed to rollback postgres migration transaction");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rurqe_migrate::migration::Migration;
    use rurqe_migrate::runner::Migrator;

    fn backend() -> SqliteMigrationBackend {
        SqliteMigrationBackend::new(rusqlite::Connection::open_in_memory().unwrap())
    }

    fn migrations(script: &str, sql: &str) -> Vec<Migration> {
        vec![Migration::parse(script, sql).unwrap()]
    }

    #[test]
    fn sqlite_backend_applies_versioned_migrations_once() {
        let mut backend = backend();
        let scripts = migrations("V1__init.sql", "CREATE TABLE users (id INTEGER);");

        let report = Migrator::new().migrate(&mut backend, &scripts).unwrap();
        assert_eq!(report.applied_count, 1);

        let report = Migrator::new().migrate(&mut backend, &scripts).unwrap();
        assert_eq!(report.applied_count, 0);

        let history = backend.read_history().unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].script, "V1__init.sql");
        assert_eq!(history[0].kind, MigrationKind::Versioned);
        assert!(history[0].success);
    }

    #[test]
    fn sqlite_backend_rolls_back_failed_migrations() {
        let mut backend = backend();
        let broken = migrations("V1__broken.sql", "CREATE TABLE oops (;");

        let error = Migrator::new()
            .migrate(&mut backend, &broken)
            .expect_err("the broken script fails to execute");
        assert!(matches!(
            error,
            rurqe_migrate::error::Error::MigrationExecutionFailed { .. }
        ));

        let history = backend.read_history().expect("history is readable");
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].script, "V1__broken.sql");
        assert!(!history[0].success);

        let blocked = Migrator::new()
            .migrate(&mut backend, &broken)
            .expect_err("the failed entry blocks the next run");
        assert!(matches!(
            blocked,
            rurqe_migrate::error::Error::FailedMigrationAlreadyApplied { .. }
        ));

        let migrator = Migrator::new();
        let fixed = migrations("V1__fixed.sql", "CREATE TABLE ok (id INTEGER);");
        let repair = migrator
            .repair(&mut backend, &fixed)
            .expect("repair purges the failed entry");
        assert_eq!(repair.removed_failed_entries, 1);

        let report = migrator
            .migrate(&mut backend, &fixed)
            .expect("the fixed script applies");
        assert_eq!(report.applied_count, 1);
    }

    #[test]
    fn sqlite_backend_reports_the_digest_of_its_catalog() {
        let mut backend = backend();
        let scripts = migrations(
            "V1__users.sql",
            "CREATE TABLE users (id INTEGER NOT NULL PRIMARY KEY, email TEXT NOT NULL);",
        );
        Migrator::new()
            .migrate(&mut backend, &scripts)
            .expect("the script applies");

        let catalog = backend
            .introspect_catalog()
            .expect("the catalog of the connection is introspected");
        assert_eq!(catalog.tables.len(), 1);
        assert_eq!(catalog.tables[0].name, "users");

        // The history table stays outside the catalog the digest describes.
        assert_eq!(
            backend
                .catalog_checksum()
                .expect("the digest of the connection"),
            Some(catalog.checksum())
        );
    }

    #[test]
    fn sqlite_transaction_introspects_the_catalog_it_sees() {
        let mut backend = backend();
        let scripts = migrations("V1__users.sql", "CREATE TABLE users (id INTEGER NOT NULL);");
        Migrator::new()
            .migrate(&mut backend, &scripts)
            .expect("the script applies");

        let mut tx = backend.begin().expect("the migration transaction opens");
        let catalog = tx
            .introspect_catalog()
            .expect("the catalog of the transaction is introspected");
        assert_eq!(catalog.tables.len(), 1);
        assert_eq!(catalog.tables[0].name, "users");

        // A change applied by the very transaction is visible to its own catalog.
        tx.execute("CREATE TABLE orders (id INTEGER NOT NULL);")
            .expect("the transaction applies the step");
        assert_eq!(
            tx.introspect_catalog()
                .expect("the catalog of the transaction is introspected")
                .tables
                .len(),
            2
        );

        tx.rollback().expect("the transaction is rolled back");
    }
}
