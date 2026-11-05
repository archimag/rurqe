//! The DDL scripts of the declarative stage and the directory they live in.

use std::path::Path;

use crate::checksum::{Checksum, ChecksumHasher};
use crate::error::Error;
use crate::migration::parse_script_attributes;
use crate::scripts::{compare_script_names, sql_scripts};

/// One DDL script of the declarative stage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StageScript {
    name: String,
    sql: String,
    transactional: bool,
}

impl StageScript {
    /// Creates a script from its file name and DDL contents.
    ///
    /// The script is transactional unless its name or its comments declare
    /// otherwise, following the same attribute parser the migration tail uses.
    pub fn new(name: impl Into<String>, sql: impl Into<String>) -> Self {
        let name = name.into();
        let sql = sql.into();
        let transactional = parse_script_attributes(&name, &sql).is_transactional();

        Self {
            name,
            sql,
            transactional,
        }
    }

    /// The file name of the script inside the stage directory.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The DDL the script applies.
    pub fn sql(&self) -> &str {
        &self.sql
    }

    /// Whether the script runs inside the transaction the cold start opens.
    pub fn is_transactional(&self) -> bool {
        self.transactional
    }

    /// Overrides whether the script runs inside a transaction.
    pub fn with_transactional(mut self, transactional: bool) -> Self {
        self.transactional = transactional;
        self
    }
}

/// The declarative stage: the DDL scripts describing the target schema.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DeclarativeStage {
    scripts: Vec<StageScript>,
}

impl DeclarativeStage {
    /// The directory holding the DDL scripts of the stage.
    pub const DIRECTORY: &str = "schema";

    /// Collects the scripts, ordering them by file name.
    ///
    /// Directories are read in whatever order the file system reports, so the
    /// scripts are ordered here to keep the applied DDL deterministic. A numeric
    /// component of a name is compared by its value, so `2_users.sql` precedes
    /// `10_orders.sql` without a leading zero.
    pub fn new(scripts: Vec<StageScript>) -> Self {
        let mut scripts = scripts;
        scripts.sort_by(|left, right| compare_script_names(left.name(), right.name()));
        Self { scripts }
    }

    /// Reads every `*.sql` file of `dir`, returning `None` when it is absent.
    ///
    /// A missing directory means "this project has no declarative stage", while
    /// an unreadable file is an error: silently skipping a script would
    /// initialise the database with an incomplete schema.
    pub fn load(dir: &Path) -> Result<Option<Self>, Error> {
        let scripts = match sql_scripts(dir)? {
            Some(scripts) => scripts,
            None => return Ok(None),
        };

        let mut stage = Vec::with_capacity(scripts.len());

        for script in scripts {
            stage.push(StageScript::new(script.name(), script.read()?));
        }

        Ok(Some(Self::new(stage)))
    }

    /// The scripts of the stage in application order.
    pub fn scripts(&self) -> &[StageScript] {
        &self.scripts
    }

    /// Whether the stage declares no DDL at all.
    pub fn is_empty(&self) -> bool {
        self.scripts.is_empty()
    }

    /// The SHA-256 digest of the stage in application order.
    ///
    /// File names and bodies are hashed in the order they are applied, so
    /// reordering the directory never changes the digest while any edit of a
    /// script does. The digest anchors a cold start: a database initialised from
    /// a diverged stage is rejected instead of being silently overwritten.
    ///
    /// The hasher is fed one piece at a time, so the DDL text of the stage is
    /// never concatenated into a second buffer to be digested.
    pub fn checksum(&self) -> Checksum {
        let mut hasher = ChecksumHasher::new();

        for script in &self.scripts {
            hasher.update(script.name().as_bytes());
            hasher.update(b"\n");
            hasher.update(script.sql().as_bytes());
            hasher.update(b"\n");
        }

        hasher.finalize()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numeric_name_components_order_by_value() {
        let stage = DeclarativeStage::new(vec![
            StageScript::new("10_orders.sql", "CREATE TABLE orders (id INTEGER);"),
            StageScript::new("2_users.sql", "CREATE TABLE users (id INTEGER);"),
            StageScript::new("1_init.sql", "CREATE SCHEMA app;"),
        ]);

        let names: Vec<&str> = stage.scripts().iter().map(StageScript::name).collect();
        assert_eq!(names, vec!["1_init.sql", "2_users.sql", "10_orders.sql"]);
    }

    #[test]
    fn checksum_digests_names_and_bodies_in_application_order() {
        let stage = DeclarativeStage::new(vec![
            StageScript::new("2_users.sql", "CREATE TABLE users (id INTEGER);"),
            StageScript::new("1_init.sql", "CREATE SCHEMA app;"),
        ]);

        let mut joined = String::new();
        for script in stage.scripts() {
            joined.push_str(script.name());
            joined.push('\n');
            joined.push_str(script.sql());
            joined.push('\n');
        }

        assert_eq!(stage.checksum(), Checksum::of(joined.as_bytes()));
    }
}
