//! Filesystem migration script loader.
//!
//! Scans migration folders (`migrations/`) for Flyway-compatible versioned
//! (`V<version>__<description>.sql`) and repeatable (`R__<description>.sql`) scripts.

use std::fs;
use std::path::Path;

use rurqe_migrate::error::Error;
use rurqe_migrate::migration::Migration;
use rurqe_migrate::runner::Migrator;

/// Reads and parses all migration `.sql` scripts from a directory, sorting them deterministically.
pub fn load_migrations_from_dir(dir: &Path) -> Result<Vec<Migration>, Error> {
    if !dir.exists() {
        return Ok(Vec::new());
    }

    let entries = fs::read_dir(dir)
        .map_err(|e| Error::Backend(format!("failed to read directory {}: {e}", dir.display())))?;

    let mut migrations = Vec::new();

    for entry in entries {
        let entry = entry
            .map_err(|e| Error::Backend(format!("failed to read directory entry: {e}")))?;
        let path = entry.path();

        if path.is_file() {
            if let Some(ext) = path.extension() {
                if ext == "sql" {
                    if let Some(file_name) = path.file_name().and_then(|n| n.to_str()) {
                        let sql = fs::read_to_string(&path).map_err(|e| {
                            Error::Backend(format!("failed to read file {}: {e}", path.display()))
                        })?;
                        let migration = Migration::parse(file_name, &sql)?;
                        migrations.push(migration);
                    }
                }
            }
        }
    }

    Ok(Migrator::sort_migrations(&migrations))
}

/// Helper to read in-memory migration file contents.
pub fn read_migration_files(scripts: &[(&str, &str)]) -> Result<Vec<Migration>, Error> {
    let mut parsed = Vec::with_capacity(scripts.len());
    for &(name, sql) in scripts {
        parsed.push(Migration::parse(name, sql)?);
    }
    Ok(Migrator::sort_migrations(&parsed))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_sorts_in_memory_scripts() {
        let scripts = [
            ("V2__orders.sql", "CREATE TABLE orders (id INT);"),
            ("V1__init.sql", "CREATE TABLE users (id INT);"),
            ("R__view.sql", "CREATE VIEW v AS SELECT 1;"),
        ];

        let migrations = read_migration_files(&scripts).expect("valid scripts");

        assert_eq!(migrations.len(), 3);
        assert_eq!(migrations[0].script, "V1__init.sql");
        assert_eq!(migrations[1].script, "V2__orders.sql");
        assert_eq!(migrations[2].script, "R__view.sql");
    }
}
