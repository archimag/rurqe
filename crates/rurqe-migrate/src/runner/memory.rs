//! An in-memory backend for deterministic execution and testing.

use crate::checksum::Checksum;
use crate::error::Error;
use crate::history::{create_schema_history_table_sql, SchemaHistoryEntry};

use super::backend::{MigrationBackend, MigrationTransaction};

/// An in-memory [`MigrationBackend`] for deterministic execution and testing.
#[derive(Debug, Default, Clone)]
pub struct InMemoryBackend {
    /// Recorded schema history entries.
    pub history: Vec<SchemaHistoryEntry>,
    /// Log of raw SQL executed.
    pub executed_sql: Vec<String>,
    /// Flag indicating whether table creation was requested.
    pub table_created: bool,
    /// Failure simulation on SQL execution.
    pub failure_script: Option<String>,
    /// Log of the scripts executed outside a transaction.
    pub non_transactional_sql: Vec<String>,
    /// Flag indicating whether exclusive migration lock is acquired.
    pub locked: bool,
    /// Digest the backend reports for the catalog it currently holds.
    pub catalog_checksum: Option<Checksum>,
    /// Fails the transaction that records a successful history entry.
    pub fail_success_entry: bool,
    /// Fails the commit of a transaction that staged a successful history entry.
    pub fail_success_commit: bool,
    /// Number of history entries visible each time a script ran outside a transaction.
    pub history_len_at_execution: Vec<usize>,
}

impl InMemoryBackend {
    /// Creates an empty in-memory backend.
    pub fn new() -> Self {
        Self::default()
    }
}

impl MigrationBackend for InMemoryBackend {
    type Transaction<'a> = InMemoryTransaction<'a>;

    fn ensure_schema_history_table(&mut self) -> Result<(), Error> {
        self.table_created = true;
        self.executed_sql
            .push(create_schema_history_table_sql().to_owned());
        Ok(())
    }

    fn read_history(&mut self) -> Result<Vec<SchemaHistoryEntry>, Error> {
        Ok(self.history.clone())
    }

    fn catalog_checksum(&mut self) -> Result<Option<Checksum>, Error> {
        Ok(self.catalog_checksum)
    }

    fn begin(&mut self) -> Result<Self::Transaction<'_>, Error> {
        let initial_history = self.history.clone();
        Ok(InMemoryTransaction {
            backend: self,
            staged_history: initial_history,
            staged_sql: Vec::new(),
        })
    }

    fn execute_non_transactional(&mut self, sql: &str) -> Result<(), Error> {
        if let Some(ref fail_sql) = self.failure_script {
            if sql.contains(fail_sql) {
                return Err(Error::Backend(format!("simulated failure on `{sql}`")));
            }
        }
        self.history_len_at_execution.push(self.history.len());
        self.non_transactional_sql.push(sql.to_owned());
        Ok(())
    }

    fn acquire_lock(&mut self) -> Result<(), Error> {
        if self.locked {
            return Err(Error::Backend("migration lock already acquired".to_owned()));
        }
        self.locked = true;
        Ok(())
    }

    fn release_lock(&mut self) -> Result<(), Error> {
        self.locked = false;
        Ok(())
    }
}

/// Active transaction for [`InMemoryBackend`].
pub struct InMemoryTransaction<'a> {
    backend: &'a mut InMemoryBackend,
    staged_history: Vec<SchemaHistoryEntry>,
    staged_sql: Vec<String>,
}

impl MigrationTransaction for InMemoryTransaction<'_> {
    fn execute(&mut self, sql: &str) -> Result<(), Error> {
        if let Some(ref fail_sql) = self.backend.failure_script {
            if sql.contains(fail_sql) {
                return Err(Error::Backend(format!("simulated failure on `{sql}`")));
            }
        }
        self.staged_sql.push(sql.to_owned());
        Ok(())
    }

    fn record_entry(&mut self, entry: &SchemaHistoryEntry) -> Result<(), Error> {
        if entry.success && self.backend.fail_success_entry {
            return Err(Error::Backend(format!(
                "simulated failure recording the applied entry of `{}`",
                entry.script
            )));
        }
        self.staged_history.push(entry.clone());
        Ok(())
    }

    fn delete_entry(&mut self, installed_rank: i64) -> Result<(), Error> {
        self.staged_history
            .retain(|e| e.installed_rank != installed_rank);
        Ok(())
    }

    fn update_checksum(&mut self, script: &str, checksum: &Checksum) -> Result<(), Error> {
        for entry in &mut self.staged_history {
            if entry.script == script {
                entry.checksum = Some(*checksum);
            }
        }
        Ok(())
    }

    fn commit(self) -> Result<(), Error> {
        if self.backend.fail_success_commit && self.staged_history.iter().any(|entry| entry.success)
        {
            return Err(Error::Backend(
                "simulated failure committing an applied history entry".to_owned(),
            ));
        }
        self.backend.history = self.staged_history;
        self.backend.executed_sql.extend(self.staged_sql);
        Ok(())
    }

    fn rollback(self) -> Result<(), Error> {
        Ok(())
    }
}
