//! The DDL scripts of the declarative stage and the directory they live in.

use std::path::Path;

use crate::checksum::Checksum;
use crate::error::Error;
use crate::migration::parse_script_attributes;
use crate::scripts::sql_scripts;

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

    /// Collects the scripts, sorting them by file name.
    ///
    /// Directories are read in whatever order the file system reports, so the
    /// scripts are ordered here to keep the applied DDL deterministic.
    pub fn new(scripts: Vec<StageScript>) -> Self {
        let mut scripts = scripts;
        scripts.sort_by(|left, right| left.name.cmp(&right.name));
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
    pub fn checksum(&self) -> Checksum {
        let mut digest = String::new();

        for script in &self.scripts {
            digest.push_str(&script.name);
            digest.push('\n');
            digest.push_str(&script.sql);
            digest.push('\n');
        }

        Checksum::of(digest.as_bytes())
    }
}
