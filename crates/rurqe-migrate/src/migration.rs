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

/// The keys of the header-comment directives disabling the transaction.
///
/// A directive is written as `<key> = false` on a comment line of its own. Both
/// keys are stored folded by [`directive_token`], so `executeInTransaction`,
/// `execute_in_transaction`, and `EXECUTE_IN_TRANSACTION` reach the same declared
/// intent.
const TRANSACTION_DIRECTIVE_KEYS: [&str; 2] = ["executeintransaction", "transactional"];

/// The value of a directive that disables the surrounding transaction.
const NON_TRANSACTIONAL_VALUE: &str = "false";

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
/// suffix (`.notx.sql` or `__notx.sql`) or one of its comment lines is exactly
/// `executeInTransaction = false`, `execute_in_transaction = false`, or
/// `transactional = false`.
pub fn parse_script_attributes(script_name: &str, sql: &str) -> ScriptAttributes {
    let named_non_transactional = script_name
        .to_lowercase()
        .ends_with(NON_TRANSACTIONAL_SUFFIX);
    let declared_non_transactional = mentions_non_transactional_directive(&comment_text(sql));

    ScriptAttributes {
        transactional: !named_non_transactional && !declared_non_transactional,
    }
}

/// Whether `comments` holds a line declaring the script as non-transactional.
///
/// The scan is line-scoped on purpose: a narrative comment that only mentions the
/// switch must not disable the surrounding transaction, so one whole comment line
/// has to be the directive and nothing else.
fn mentions_non_transactional_directive(comments: &str) -> bool {
    comments.lines().any(is_non_transactional_directive)
}

/// Whether `line` is exactly one `<key> = false` directive line.
///
/// The key is everything written before the first `=`, the value everything
/// written after it, and both are folded by [`directive_token`]. A line carrying
/// any extra word — which is exactly what a prose mention adds — therefore stops
/// matching, while comment decoration and trailing punctuation never do.
fn is_non_transactional_directive(line: &str) -> bool {
    let Some((key, value)) = line.split_once('=') else {
        return false;
    };

    let key = directive_token(key);
    let value = directive_token(value);

    TRANSACTION_DIRECTIVE_KEYS.contains(&key.as_str()) && value == NON_TRANSACTIONAL_VALUE
}

/// Folds `text` into the token shape directives are compared in.
///
/// Only alphanumeric characters survive, and they are lowercased, so whitespace,
/// underscores, and the decoration a comment line may carry (`*`, `--`, `#`) never
/// change the comparison: `execute_in_transaction = FALSE`,
/// `executeInTransaction=false`, and `* transactional = false;` all reduce to the
/// same key and value pair.
fn directive_token(text: &str) -> String {
    text.chars()
        .filter(|character| character.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// The state of the scanner reading the SQL text.
#[derive(Clone, Copy, PartialEq, Eq)]
enum CommentScan {
    /// Ordinary SQL text.
    Sql,
    /// After `--`, until the end of the line.
    LineComment,
    /// Inside `/* ... */`, tracking the nesting depth.
    BlockComment,
    /// Inside a `'...'` literal.
    TextLiteral,
    /// Inside a `"..."` identifier.
    QuotedIdentifier,
    /// Inside a `$tag$ ... $tag$` dollar-quoted body.
    DollarQuoted,
}

/// Collects the text of every comment in `sql`, skipping literals.
///
/// Every comment line reaches the buffer on a line of its own: a line comment is
/// terminated by a newline, and the closing boundary of a block comment — as well
/// as both boundaries of a nested one — push a separator too. The directive scan
/// therefore compares one whole comment line at a time and never glues a
/// directive to the remark written above it or around a nested comment.
///
/// String literals, quoted identifiers, and dollar-quoted bodies are tracked so a
/// directive written inside data never disables the surrounding transaction. The
/// dollar-quote form is the standard way to embed `plpgsql` function bodies in a
/// PostgreSQL migration, and the quoted body routinely contains `--` and
/// `/* ... */` sequences that are data rather than comments.
///
/// Block comments nest the way PostgreSQL nests them, so `/* a /* b */ c */` is a
/// single comment. Only the outermost level contributes directive text: the body
/// of a nested comment is skipped, while text written between nested comments is
/// collected on a line of its own, because entering and leaving a nested comment
/// both break the surrounding text.
///
/// The scanner walks the text character by character and never indexes outside
/// the collected characters, so malformed SQL cannot make it panic.
fn comment_text(sql: &str) -> String {
    let characters: Vec<char> = sql.chars().collect();
    let mut comments = String::new();
    let mut state = CommentScan::Sql;
    let mut block_depth = 0usize;
    let mut dollar_tag: Vec<char> = Vec::new();
    let mut index = 0usize;

    while let Some(&current) = characters.get(index) {
        match state {
            CommentScan::Sql => match current {
                '-' if characters.get(index + 1) == Some(&'-') => {
                    index += 1;
                    state = CommentScan::LineComment;
                }
                '/' if characters.get(index + 1) == Some(&'*') => {
                    index += 1;
                    block_depth = 1;
                    state = CommentScan::BlockComment;
                }
                '\'' => state = CommentScan::TextLiteral,
                '"' => state = CommentScan::QuotedIdentifier,
                '$' => {
                    if let Some(delimiter) = dollar_quote_opening(&characters, index) {
                        index += delimiter.len() - 1;
                        dollar_tag = delimiter;
                        state = CommentScan::DollarQuoted;
                    }
                }
                _ => {}
            },
            CommentScan::LineComment => {
                if current == '\n' {
                    // The end of the remark is kept as a separator, so two
                    // consecutive comment lines never merge into one scanned
                    // line and a directive stays isolated.
                    comments.push('\n');
                    state = CommentScan::Sql;
                } else {
                    comments.push(current);
                }
            }
            CommentScan::BlockComment => {
                if current == '/' && characters.get(index + 1) == Some(&'*') {
                    index += 1;
                    block_depth = block_depth.saturating_add(1);
                    // Entering a nested comment breaks the surrounding text into
                    // two lines, so a directive written around it still occupies
                    // a line of its own.
                    comments.push('\n');
                } else if current == '*' && characters.get(index + 1) == Some(&'/') {
                    index += 1;
                    block_depth = block_depth.saturating_sub(1);
                    // Closing the comment — nested or outermost — separates the
                    // collected text from whatever is written after it.
                    comments.push('\n');
                    if block_depth == 0 {
                        state = CommentScan::Sql;
                    }
                } else if block_depth == 1 {
                    comments.push(current);
                }
            }
            CommentScan::TextLiteral => {
                if current == '\'' {
                    if characters.get(index + 1) == Some(&'\'') {
                        index += 1;
                    } else {
                        state = CommentScan::Sql;
                    }
                }
            }
            CommentScan::QuotedIdentifier => {
                if current == '"' {
                    if characters.get(index + 1) == Some(&'"') {
                        index += 1;
                    } else {
                        state = CommentScan::Sql;
                    }
                }
            }
            CommentScan::DollarQuoted => {
                if current == '$'
                    && !dollar_tag.is_empty()
                    && starts_with_at(&characters, index, &dollar_tag)
                {
                    index += dollar_tag.len() - 1;
                    dollar_tag.clear();
                    state = CommentScan::Sql;
                }
            }
        }

        index += 1;
    }

    comments
}

/// Returns the `$tag$` delimiter opening a dollar-quoted body at `start`.
///
/// The character at `start` must be the leading `$`. A tag, when present, follows
/// identifier rules: it starts with a letter or an underscore and continues with
/// letters, digits, or underscores. A `$` glued to the end of an identifier is
/// not a delimiter, and neither is a positional placeholder such as `$1`, so both
/// stay ordinary SQL text.
fn dollar_quote_opening(characters: &[char], start: usize) -> Option<Vec<char>> {
    let preceding = start.checked_sub(1).and_then(|at| characters.get(at));
    if matches!(preceding, Some(&character) if character.is_alphanumeric() || character == '_') {
        return None;
    }

    let mut end = start + 1;
    match characters.get(end) {
        Some(&'$') => {}
        Some(&first) if first.is_alphabetic() || first == '_' => {
            end += 1;
            while let Some(&next) = characters.get(end) {
                if next == '$' {
                    break;
                }
                if !(next.is_alphanumeric() || next == '_') {
                    return None;
                }
                end += 1;
            }
        }
        _ => return None,
    }

    Some(characters.get(start..=end)?.to_vec())
}

/// Whether `characters` holds `needle` starting exactly at `start`.
fn starts_with_at(characters: &[char], start: usize, needle: &[char]) -> bool {
    needle
        .iter()
        .enumerate()
        .all(|(offset, expected)| characters.get(start.wrapping_add(offset)) == Some(expected))
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
    fn every_declared_directive_matches_a_canonical_sample() {
        // One representative comment per spelling the engine documents, paired
        // with the key it has to fold onto.
        let spellings = [
            ("-- executeInTransaction=false\n", "executeintransaction"),
            ("-- executeInTransaction = false\n", "executeintransaction"),
            ("-- execute_in_transaction=false\n", "executeintransaction"),
            (
                "-- execute_in_transaction = false\n",
                "executeintransaction",
            ),
            (
                "-- EXECUTE_IN_TRANSACTION = FALSE\n",
                "executeintransaction",
            ),
            ("-- transactional = false\n", "transactional"),
            ("/* TRANSACTIONAL   =   FALSE */\n", "transactional"),
        ];

        for (comment, key) in spellings {
            assert!(
                TRANSACTION_DIRECTIVE_KEYS.contains(&key),
                "`{key}` must be declared by the directive table"
            );
            assert!(
                mentions_non_transactional_directive(comment),
                "`{comment}` must declare `{key} = false`"
            );
        }

        for key in TRANSACTION_DIRECTIVE_KEYS {
            assert_eq!(directive_token(key), key, "`{key}` must be stored folded");
            assert!(
                mentions_non_transactional_directive(&format!("-- {key} = false")),
                "`{key}` must match at least one canonical comment"
            );
        }
    }

    #[test]
    fn a_directive_mentioned_in_prose_keeps_the_transaction() {
        // A narrative remark mentioning the switch is not a directive: the line
        // has to carry the key and the value and nothing else.
        for sql in [
            "-- the executeInTransaction=false switch is required for concurrent indexes\nCREATE TABLE users ();",
            "-- see execute_in_transaction = false in the runbook\nCREATE TABLE users ();",
            "-- set transactional = false here when the index needs it\nCREATE TABLE users ();",
            "-- `transactional = false` disables the surrounding transaction\nCREATE TABLE users ();",
            "-- transactional = false unless the table is large\nCREATE TABLE users ();",
            "/*\n * Normally this script declares transactional = false,\n * but it does not need to.\n */\nCREATE TABLE users ();",
        ] {
            let migration = Migration::parse("V1__init.sql", sql).unwrap();
            assert!(
                migration.is_transactional(),
                "`{sql}` must stay transactional"
            );
        }
    }

    #[test]
    fn a_directive_stays_isolated_from_the_remark_above_it() {
        // Line-scoped scanning: a remark directly above the directive must not
        // merge with it, and the directive itself still has to win.
        for sql in [
            "-- concurrent index build\n-- transactional = false\nCREATE INDEX CONCURRENTLY users_email_idx ON users (email);",
            "/* first remark */\n/* transactional = false */\nCREATE TABLE users ();",
            "-- note\n  --   execute_in_transaction   =   false\nCREATE TABLE users ();",
        ] {
            let migration = Migration::parse("V1__init.sql", sql).unwrap();
            assert!(
                !migration.is_transactional(),
                "`{sql}` must be non-transactional"
            );
        }

        // Two remarks whose folded concatenation would spell the directive stay
        // harmless because they are separate lines.
        for sql in [
            "-- transactional\n-- = false\nCREATE TABLE users ();",
            "-- execute_in_transaction =\n-- false\nCREATE TABLE users ();",
        ] {
            let migration = Migration::parse("V1__init.sql", sql).unwrap();
            assert!(
                migration.is_transactional(),
                "`{sql}` must stay transactional"
            );
        }
    }

    #[test]
    fn a_directive_survives_comment_decoration() {
        // Decoration and trailing punctuation are not words, so an otherwise
        // isolated directive keeps matching.
        for sql in [
            "--- transactional = false;\nCREATE TABLE users ();",
            "/*\n * executeInTransaction = false\n */\nCREATE TABLE users ();",
            "/* ** transactional = false ** */\nCREATE TABLE users ();",
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

    #[test]
    fn a_directive_inside_a_dollar_quoted_body_is_ignored() {
        for sql in [
            "CREATE FUNCTION f() RETURNS void AS $$\n-- execute_in_transaction = false\n$$ LANGUAGE plpgsql;",
            "CREATE FUNCTION f() RETURNS void AS $$\n/* transactional = false */\n$$ LANGUAGE plpgsql;",
            "CREATE FUNCTION f() RETURNS void AS $body$\n-- executeInTransaction = false\n$body$ LANGUAGE plpgsql;",
            "CREATE FUNCTION f() RETURNS text AS $body$ SELECT '-- transactional = false'; $body$ LANGUAGE sql;",
            "INSERT INTO notes (body) VALUES ($tag$ -- execute_in_transaction=false $tag$);",
            "INSERT INTO notes (body) VALUES ($$$$);",
        ] {
            let migration = Migration::parse("V1__init.sql", sql).unwrap();
            assert!(
                migration.is_transactional(),
                "`{sql}` must stay transactional"
            );
        }
    }

    #[test]
    fn a_directive_after_a_dollar_quoted_body_is_honoured() {
        let sql = "CREATE FUNCTION f() RETURNS void AS $$ SELECT 1; $$ LANGUAGE sql;\n-- transactional = false\n";
        let migration = Migration::parse("V1__init.sql", sql).unwrap();
        assert!(!migration.is_transactional());
    }

    #[test]
    fn positional_placeholders_never_open_a_dollar_quote() {
        let sql = "UPDATE users SET name = $1 WHERE id = $2;\n-- transactional = false\n";
        let migration = Migration::parse("V1__init.sql", sql).unwrap();
        assert!(!migration.is_transactional());
    }

    #[test]
    fn a_directive_inside_a_nested_block_comment_is_ignored() {
        for sql in [
            "/* /* execute_in_transaction = false */ */\nCREATE TABLE users ();",
            "/* outer /* executeInTransaction = false */ still outer */\nCREATE TABLE users ();",
            "/*\n  /* transactional = false */\n*/\nCREATE TABLE users ();",
        ] {
            let migration = Migration::parse("V1__init.sql", sql).unwrap();
            assert!(
                migration.is_transactional(),
                "`{sql}` must stay transactional"
            );
        }
    }

    #[test]
    fn nesting_block_comments_keeps_the_outer_directive() {
        for sql in [
            "/* transactional = false /* nested */ */\nCREATE TABLE users ();",
            "/* nested /* inner */ execute_in_transaction = false */\nCREATE TABLE users ();",
        ] {
            let migration = Migration::parse("V1__init.sql", sql).unwrap();
            assert!(
                !migration.is_transactional(),
                "`{sql}` must be non-transactional"
            );
        }
    }

    #[test]
    fn malformed_sql_never_panics_the_scanner() {
        // Unterminated constructs must leave the scanner in a stable state and
        // still honour a directive declared before them.
        for sql in [
            "/* transactional = false",
            "-- transactional = false",
            "CREATE FUNCTION f() AS $$ -- transactional = false",
            "SELECT 'unterminated -- transactional = false",
            "SELECT \"unterminated -- transactional = false",
            "",
            "/*/",
            "$",
        ] {
            let _ = Migration::parse("V1__init.sql", sql).unwrap();
        }
    }
}
