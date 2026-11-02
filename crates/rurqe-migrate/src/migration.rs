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
use crate::history::validate_metadata;
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

/// A parsed migration script.
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
    ///
    /// The parsed metadata is checked against the column widths of the history
    /// table, so a version or description that cannot be recorded is refused
    /// before the script reaches a backend.
    pub fn parse(script: &str, sql: &str) -> Result<Self, Error> {
        let (kind, version, description) = parse_script_name(script)?;
        validate_metadata(script, version.as_ref(), &description)?;
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
mod tests;
