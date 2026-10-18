//! Parsed migration scripts.
//!
//! A script name encodes everything the migration engine needs before touching
//! the database: the kind of the migration, its version, its description, and,
//! once the file is read, the [`Checksum`] of its SQL contents.
//!
//! ```text
//! V2_1__add_orders.sql   -> Versioned, version 2.1, description "add orders"
//! R__views_and_funcs.sql -> Repeatable, description "views and funcs"
//! ```

use crate::checksum::Checksum;
use crate::error::Error;
use crate::version::MigrationVersion;

/// The file extension required by migration scripts.
const SCRIPT_EXTENSION: &str = ".sql";

/// The separator between the version and the description of a script name.
const DESCRIPTION_SEPARATOR: &str = "__";

/// The kind of a migration script.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MigrationKind {
    /// A `V{version}__{description}.sql` script applied exactly once.
    Versioned,
    /// An `R__{description}.sql` script re-applied whenever its checksum changes.
    Repeatable,
}

impl MigrationKind {
    /// The stable text stored in the history table.
    pub fn as_str(&self) -> &'static str {
        match self {
            MigrationKind::Versioned => "VERSIONED",
            MigrationKind::Repeatable => "REPEATABLE",
        }
    }
}

/// A migration script and its metadata.
///
/// Versioned migrations carry a [`MigrationVersion`] and are applied once in
/// ascending version order; repeatable migrations have no version at all and are
/// re-applied whenever their [`Checksum`] differs from the recorded one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Migration {
    /// The version of a versioned migration, `None` for a repeatable one.
    pub version: Option<MigrationVersion>,
    /// The human-readable description read from the script name.
    pub description: String,
    /// The script name, including its extension.
    pub script: String,
    /// Whether the migration is versioned or repeatable.
    pub kind: MigrationKind,
    /// The SHA-256 digest of the script contents.
    pub checksum: Checksum,
}

impl Migration {
    /// Parses the script name `script` and digests its SQL contents `sql`.
    ///
    /// The name must be `<prefix><rest>.sql`, where the prefix is `V` for a
    /// versioned migration followed by `<version>__` or `R` for a repeatable one
    /// followed by `__`. Underscores inside the description are rendered as
    /// spaces, matching the history table contents.
    pub fn parse(script: &str, sql: &str) -> Result<Self, Error> {
        let (kind, version, description) = parse_script_name(script)?;

        Ok(Self {
            version,
            description,
            script: script.to_owned(),
            kind,
            checksum: Checksum::of(sql.as_bytes()),
        })
    }

    /// Whether the migration is versioned and therefore applied exactly once.
    pub fn is_versioned(&self) -> bool {
        self.kind == MigrationKind::Versioned
    }
}

fn parse_script_name(
    script: &str,
) -> Result<(MigrationKind, Option<MigrationVersion>, String), Error> {
    let invalid = || Error::InvalidScriptName {
        script: script.to_owned(),
    };

    let stem = script.strip_suffix(SCRIPT_EXTENSION).ok_or_else(invalid)?;

    let (kind, rest) = if let Some(rest) = stem.strip_prefix('V') {
        (MigrationKind::Versioned, rest)
    } else if let Some(rest) = stem.strip_prefix('R') {
        (MigrationKind::Repeatable, rest)
    } else {
        return Err(invalid());
    };

    let (version, description) = match kind {
        MigrationKind::Versioned => {
            let (version, description) = rest.split_once(DESCRIPTION_SEPARATOR).ok_or_else(invalid)?;
            if version.is_empty() {
                return Err(invalid());
            }
            let version = version.parse::<MigrationVersion>()?;
            (Some(version), description)
        }
        MigrationKind::Repeatable => {
            let description = rest.strip_prefix(DESCRIPTION_SEPARATOR).ok_or_else(invalid)?;
            (None, description)
        }
    };

    Ok((kind, version, description.replace('_', " ")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versioned_script_yields_version_and_description() {
        let migration =
            Migration::parse("V2_1__add_orders.sql", "CREATE TABLE orders ();").unwrap();

        assert_eq!(migration.kind, MigrationKind::Versioned);
        assert_eq!(migration.version.as_ref().unwrap().to_string(), "2.1");
        assert_eq!(migration.description, "add orders");
        assert_eq!(migration.script, "V2_1__add_orders.sql");
        assert_eq!(migration.checksum, Checksum::of(b"CREATE TABLE orders ();"));
        assert!(migration.is_versioned());
    }

    #[test]
    fn repeatable_script_has_no_version() {
        let migration = Migration::parse("R__views_and_functions.sql", "SELECT 1;").unwrap();

        assert_eq!(migration.kind, MigrationKind::Repeatable);
        assert_eq!(migration.version, None);
        assert_eq!(migration.description, "views and functions");
        assert!(!migration.is_versioned());
    }

    #[test]
    fn dot_separated_versions_are_equivalent_to_underscored_ones() {
        let dotted = Migration::parse("V1.0.0__init.sql", "SELECT 1;").unwrap();
        let underscored = Migration::parse("V1_0_0__init.sql", "SELECT 1;").unwrap();

        assert_eq!(dotted.version, underscored.version);
        assert_eq!(dotted.checksum, underscored.checksum);
    }

    #[test]
    fn checksum_follows_the_script_contents() {
        let original = Migration::parse("V1__init.sql", "SELECT 1;").unwrap();
        let edited = Migration::parse("V1__init.sql", "SELECT 2;").unwrap();

        assert_ne!(original.checksum, edited.checksum);
        assert_eq!(original.checksum.to_hex().len(), 64);
    }

    #[test]
    fn malformed_script_names_are_rejected() {
        for script in [
            "init.sql",
            "V1_init.sql",
            "V__init.sql",
            "V1__init.txt",
            "V1__init",
            "Rviews.sql",
            "R1__init.sql",
            "X1__init.sql",
        ] {
            assert_eq!(
                Migration::parse(script, "SELECT 1;"),
                Err(Error::InvalidScriptName {
                    script: script.to_owned()
                }),
                "`{script}` must be rejected"
            );
        }
    }

    #[test]
    fn malformed_version_is_reported_separately() {
        assert_eq!(
            Migration::parse("V1a__init.sql", "SELECT 1;"),
            Err(Error::InvalidVersion {
                version: "1a".to_owned()
            })
        );
    }

    #[test]
    fn kind_text_is_stable() {
        assert_eq!(MigrationKind::Versioned.as_str(), "VERSIONED");
        assert_eq!(MigrationKind::Repeatable.as_str(), "REPEATABLE");
    }
}
