//! Filesystem migration script loader.
//!
//! Scans migration folders (`migrations/`) for Flyway-compatible versioned
//! (`V<version>__<description>.sql`) and repeatable (`R__<description>.sql`) scripts.

use std::path::Path;

use rurqe_migrate::error::Error;
use rurqe_migrate::migration::Migration;
use rurqe_migrate::runner::Migrator;
use rurqe_migrate::scripts::sql_scripts;

/// Reads and parses all migration `.sql` scripts from a directory, sorting them deterministically.
///
/// A missing directory holds no migrations, while a script that cannot be read or
/// whose name follows no convention is reported instead of being skipped: a
/// silently ignored script would apply an incomplete migration history.
pub fn load_migrations_from_dir(dir: &Path) -> Result<Vec<Migration>, Error> {
    let scripts = match sql_scripts(dir)? {
        Some(scripts) => scripts,
        None => return Ok(Vec::new()),
    };

    let mut migrations = Vec::with_capacity(scripts.len());

    for script in scripts {
        let sql = script.read()?;
        migrations.push(Migration::parse(script.name(), &sql)?);
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

    #[test]
    fn parses_the_scripts_of_a_directory() {
        let dir =
            std::env::temp_dir().join(format!("rurqe-loader-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create the migration directory");
        std::fs::write(dir.join("V1__init.sql"), "CREATE TABLE users (id INT);")
            .expect("write the script");
        std::fs::write(dir.join("V2__orders.sql"), "CREATE TABLE orders (id INT);")
            .expect("write the script");
        std::fs::write(dir.join("notes.txt"), "not a script").expect("write the note");

        let migrations = load_migrations_from_dir(&dir).expect("valid scripts");
        let _ = std::fs::remove_dir_all(&dir);

        let names: Vec<&str> = migrations
            .iter()
            .map(|migration| migration.script.as_str())
            .collect();
        assert_eq!(names, vec!["V1__init.sql", "V2__orders.sql"]);
    }

    #[test]
    fn a_missing_directory_holds_no_migrations() {
        let dir = std::env::temp_dir().join(format!(
            "rurqe-loader-absent-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);

        assert!(load_migrations_from_dir(&dir)
            .expect("a missing directory is empty")
            .is_empty());
    }
}
