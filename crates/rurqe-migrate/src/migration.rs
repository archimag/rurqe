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

/// The script-name suffix marking a migration as non-transactional.
const NON_TRANSACTIONAL_SUFFIX: &str = "notx.sql";

/// The header-comment directives disabling the surrounding transaction.
///
/// Every entry is normalized — lowercase, without whitespace or underscores — so
/// `executeInTransaction = false`, `execute_in_transaction=false`, and
/// `transactional = false` all match the same declared intent.
const NON_TRANSACTIONAL_DIRECTIVES: [&str; 3] = [
    "executeintransaction=false",
    "execute_intransaction=false",
    "transactional=false",
];

/// The execution attributes declared by a migration script.
///
/// Attributes are read from the script name and its header comments before the
/// script reaches the database: a script marked non-transactional must run on its
/// own, outside the transaction the runner opens for regular scripts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScriptAttributes {
    /// Whether the script runs inside the transaction the runner opens.
    pub transactional: bool,
}

impl Default for ScriptAttributes {
    fn default() -> Self {
        Self {
            transactional: true,
        }
    }
}

impl ScriptAttributes {
    /// Whether the script runs inside the transaction the runner opens.
    pub fn is_transactional(&self) -> bool {
        self.transactional
    }
}

/// The kind of a migration script.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MigrationKind {
    /// A `V{version}__{description}.sql` script applied exactly once.
    Versioned,
    /// An `R__{description}.sql` script re-applied whenever its checksum changes.
    Repeatable,
    /// A synthetic baseline entry recording a starting state.
    Baseline,
}

impl MigrationKind {
    /// The stable text stored in the history table.
    pub fn as_str(&self) -> &'static str {
        match self {
            MigrationKind::Versioned => "VERSIONED",
            MigrationKind::Repeatable => "REPEATABLE",
            MigrationKind::Baseline => "BASELINE",
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
    /// Whether the migration is versioned, repeatable, or a baseline marker.
    pub kind: MigrationKind,
    /// The SHA-256 digest of the script contents.
    pub checksum: Checksum,
    /// The SQL statements of the migration script.
    pub sql: String,
    /// Whether the script runs inside the transaction the runner opens.
    pub transactional: bool,
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
        let attributes = parse_script_attributes(script, sql);

        Ok(Self {
            version,
            description,
            script: script.to_owned(),
            kind,
            checksum: Checksum::of(sql.as_bytes()),
            sql: sql.to_owned(),
            transactional: attributes.transactional,
        })
    }

    /// Whether the script runs inside the transaction the runner opens.
    pub fn is_transactional(&self) -> bool {
        self.transactional
    }

    /// Whether the migration is versioned and therefore applied exactly once.
    pub fn is_versioned(&self) -> bool {
        self.kind == MigrationKind::Versioned
    }

    /// Whether the migration is repeatable.
    pub fn is_repeatable(&self) -> bool {
        self.kind == MigrationKind::Repeatable
    }
}

/// Reads the execution attributes declared by `script_name` and its SQL text.
///
/// A script is transactional unless its name carries the non-transactional
/// suffix (`.notx.sql` or `__notx.sql`) or one of its comments declares
/// `executeInTransaction=false`, `execute_in_transaction=false`, or
/// `transactional=false`.
pub fn parse_script_attributes(script_name: &str, sql: &str) -> ScriptAttributes {
    let named_non_transactional = script_name
        .to_lowercase()
        .ends_with(NON_TRANSACTIONAL_SUFFIX);
    let declared_non_transactional = mentions_non_transactional_directive(&comment_text(sql));

    ScriptAttributes {
        transactional: !named_non_transactional && !declared_non_transactional,
    }
}

/// Whether `comments` declares the script as non-transactional.
fn mentions_non_transactional_directive(comments: &str) -> bool {
    let normalized: String = comments
        .chars()
        .filter(|character| !character.is_whitespace() && *character != '_')
        .flat_map(char::to_lowercase)
        .collect();

    NON_TRANSACTIONAL_DIRECTIVES
        .iter()
        .any(|directive| normalized.contains(directive))
}

/// The state of the scanner reading the SQL text.
#[derive(Clone, Copy, PartialEq, Eq)]
enum CommentScan {
    /// Ordinary SQL text.
    Sql,
    /// After `--`, until the end of the line.
    LineComment,
    /// Inside `/* ... */`.
    BlockComment,
    /// Inside a `'...'` literal.
    TextLiteral,
    /// Inside a `"..."` identifier.
    QuotedIdentifier,
}

/// Collects the text of every comment in `sql`, skipping literals.
///
/// String literals and quoted identifiers are tracked so a directive written
/// inside data never disables the surrounding transaction, and the scanner walks
/// the text character by character to stay panic-free on malformed SQL.
fn comment_text(sql: &str) -> String {
    let mut comments = String::new();
    let mut characters = sql.chars().peekable();
    let mut state = CommentScan::Sql;

    while let Some(current) = characters.next() {
        match state {
            CommentScan::Sql => match current {
                '-' if characters.peek() == Some(&'-') => {
                    characters.next();
                    state = CommentScan::LineComment;
                }
                '/' if characters.peek() == Some(&'*') => {
                    characters.next();
                    state = CommentScan::BlockComment;
                }
                '\'' => state = CommentScan::TextLiteral,
                '"' => state = CommentScan::QuotedIdentifier,
                _ => {}
            },
            CommentScan::LineComment => {
                if current == '\n' {
                    state = CommentScan::Sql;
                } else {
                    comments.push(current);
                }
            }
            CommentScan::BlockComment => {
                if current == '*' && characters.peek() == Some(&'/') {
                    characters.next();
                    state = CommentScan::Sql;
                } else {
                    comments.push(current);
                }
            }
            CommentScan::TextLiteral => {
                if current == '\'' {
                    if characters.peek() == Some(&'\'') {
                        characters.next();
                    } else {
                        state = CommentScan::Sql;
                    }
                }
            }
            CommentScan::QuotedIdentifier => {
                if current == '"' {
                    if characters.peek() == Some(&'"') {
                        characters.next();
                    } else {
                        state = CommentScan::Sql;
                    }
                }
            }
        }
    }

    comments
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
            let (version, description) =
                rest.split_once(DESCRIPTION_SEPARATOR).ok_or_else(invalid)?;
            if version.is_empty() {
                return Err(invalid());
            }
            let version = version.parse::<MigrationVersion>()?;
            (Some(version), description)
        }
        MigrationKind::Repeatable => {
            let description = rest
                .strip_prefix(DESCRIPTION_SEPARATOR)
                .ok_or_else(invalid)?;
            (None, description)
        }
        // The kind is derived from the script-name prefix above, so a baseline
        // marker can never reach this point; report a malformed name instead of
        // panicking on input the parser does not recognise.
        MigrationKind::Baseline => return Err(invalid()),
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
        assert!(migration.is_repeatable());
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
        assert_eq!(MigrationKind::Baseline.as_str(), "BASELINE");
    }

    #[test]
    fn scripts_are_transactional_by_default() {
        let attributes = parse_script_attributes("V1__init.sql", "CREATE TABLE users ();");
        assert_eq!(attributes, ScriptAttributes::default());
        assert!(attributes.is_transactional());

        let migration = Migration::parse("V1__init.sql", "CREATE TABLE users ();").unwrap();
        assert!(migration.transactional);
        assert!(migration.is_transactional());
    }

    #[test]
    fn a_header_directive_disables_the_transaction() {
        for sql in [
            "-- executeInTransaction=false\nCREATE TABLE users ();",
            "-- executeInTransaction = false\nCREATE TABLE users ();",
            "--   EXECUTEINTRANSACTION   =   FALSE\nCREATE TABLE users ();",
            "-- execute_in_transaction=false\nCREATE TABLE users ();",
            "-- execute_in_transaction = false\nCREATE TABLE users ();",
            "-- transactional = false\nCREATE TABLE users ();",
            "/* executeInTransaction=false */\nCREATE TABLE users ();",
            "/*\n * execute_in_transaction = false\n */\nCREATE TABLE users ();",
            "CREATE TABLE users ();\n-- transactional=FALSE\n",
        ] {
            let migration = Migration::parse("V1__init.sql", sql).unwrap();
            assert!(
                !migration.is_transactional(),
                "`{sql}` must be non-transactional"
            );
        }
    }

    #[test]
    fn an_unrelated_directive_keeps_the_transaction() {
        for sql in [
            "-- transactional = true\nCREATE TABLE users ();",
            "-- executeInTransaction=true\nCREATE TABLE users ();",
            "-- this migration runs transactionally\nCREATE TABLE users ();",
        ] {
            let migration = Migration::parse("V1__init.sql", sql).unwrap();
            assert!(
                migration.is_transactional(),
                "`{sql}` must stay transactional"
            );
        }
    }

    #[test]
    fn a_directive_inside_data_is_ignored() {
        for sql in [
            "INSERT INTO notes (body) VALUES ('-- executeInTransaction=false');",
            "INSERT INTO notes (body) VALUES ('/* transactional=false */');",
            "CREATE TABLE \"-- execute_in_transaction=false\" ();",
        ] {
            let migration = Migration::parse("V1__init.sql", sql).unwrap();
            assert!(
                migration.is_transactional(),
                "`{sql}` must stay transactional"
            );
        }
    }

    #[test]
    fn a_script_name_suffix_disables_the_transaction() {
        for script in [
            "V1__init.notx.sql",
            "V1__init__notx.sql",
            "V1__init.NOTX.sql",
            "R__views.notx.sql",
        ] {
            let migration = Migration::parse(script, "CREATE TABLE users ();").unwrap();
            assert!(
                !migration.is_transactional(),
                "`{script}` must be non-transactional"
            );
        }
    }

    #[test]
    fn the_suffix_alone_never_matches_a_foreign_name() {
        let attributes = parse_script_attributes("V1__init.sql", "SELECT 1;");
        assert!(attributes.transactional);

        let attributes = parse_script_attributes("V1__init_notx.sql.bak", "SELECT 1;");
        assert!(attributes.transactional);
    }
}
