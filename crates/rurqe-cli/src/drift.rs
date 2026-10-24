//! Deterministic schema drift detection against `schema.lock` snapshots.
//!
//! Compares a live introspected database catalog against the canonical `schema.lock`
//! file and `pub const SCHEMA_HASH` embedded in generated Rust code. Emits
//! actionable, fail-fast diagnostic error reports when code and database diverge.

use core::fmt;
use std::collections::{BTreeMap, BTreeSet};

use rurqe_migrate::catalog::{
    CheckDef, ColumnDef, EnumDef, IndexDef, SchemaCatalog, TableDef, TriggerDef, ViewDef,
};

/// Structural schema diff describing exact discrepancies.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SchemaDiff {
    pub missing_tables: Vec<String>,
    pub extra_tables: Vec<String>,
    pub table_diffs: BTreeMap<String, TableDiff>,
    pub missing_enums: Vec<String>,
    pub extra_enums: Vec<String>,
    pub altered_enums: Vec<String>,
    pub missing_views: Vec<String>,
    pub extra_views: Vec<String>,
    pub altered_views: Vec<String>,
    pub missing_triggers: Vec<String>,
    pub extra_triggers: Vec<String>,
    pub altered_triggers: Vec<String>,
}

impl SchemaDiff {
    /// Whether any differences were detected.
    pub fn is_empty(&self) -> bool {
        self.missing_tables.is_empty()
            && self.extra_tables.is_empty()
            && self.table_diffs.is_empty()
            && self.missing_enums.is_empty()
            && self.extra_enums.is_empty()
            && self.altered_enums.is_empty()
            && self.missing_views.is_empty()
            && self.extra_views.is_empty()
            && self.altered_views.is_empty()
            && self.missing_triggers.is_empty()
            && self.extra_triggers.is_empty()
            && self.altered_triggers.is_empty()
    }

    /// Renders every discrepancy as an indented bullet, one line per finding.
    ///
    /// The traversal lives here once and takes the `wording` of the verifier, so
    /// the drift report and the golden-equation report never describe the same
    /// finding with two drifting implementations.
    pub fn write_details(&self, f: &mut fmt::Formatter<'_>, wording: DiffWording) -> fmt::Result {
        let DiffWording { expected, live } = wording;

        for enum_name in &self.missing_enums {
            writeln!(
                f,
                "  - Enum `{enum_name}` is declared in {expected} but missing from {live}."
            )?;
        }
        for enum_name in &self.extra_enums {
            writeln!(
                f,
                "  - Enum `{enum_name}` exists in {live} but is missing from {expected}."
            )?;
        }
        for enum_name in &self.altered_enums {
            writeln!(
                f,
                "  - Enum `{enum_name}` definition differs between {live} and {expected}."
            )?;
        }
        for view_name in &self.missing_views {
            writeln!(
                f,
                "  - View `{view_name}` is declared in {expected} but missing from {live}."
            )?;
        }
        for view_name in &self.extra_views {
            writeln!(
                f,
                "  - View `{view_name}` exists in {live} but is missing from {expected}."
            )?;
        }
        for view_name in &self.altered_views {
            writeln!(
                f,
                "  - View `{view_name}` definition differs between {live} and {expected}."
            )?;
        }
        for trigger_name in &self.missing_triggers {
            writeln!(
                f,
                "  - Trigger `{trigger_name}` is declared in {expected} but missing from {live}."
            )?;
        }
        for trigger_name in &self.extra_triggers {
            writeln!(
                f,
                "  - Trigger `{trigger_name}` exists in {live} but is missing from {expected}."
            )?;
        }
        for trigger_name in &self.altered_triggers {
            writeln!(
                f,
                "  - Trigger `{trigger_name}` definition differs between {live} and {expected}."
            )?;
        }
        for table in &self.missing_tables {
            writeln!(
                f,
                "  - Table `{table}` is declared in {expected} but missing from {live}."
            )?;
        }
        for table in &self.extra_tables {
            writeln!(
                f,
                "  - Table `{table}` exists in {live} but is missing from {expected}."
            )?;
        }
        for (table, diff) in &self.table_diffs {
            for col in &diff.missing_columns {
                writeln!(
                    f,
                    "  - Table `{table}`: column `{col}` is in {expected} but missing from {live}."
                )?;
            }
            for col in &diff.extra_columns {
                writeln!(
                    f,
                    "  - Table `{table}`: column `{col}` exists in {live} but is missing from {expected}."
                )?;
            }
            for col in &diff.altered_columns {
                writeln!(
                    f,
                    "  - Table `{table}`: column `{col}` definition differs between {live} and {expected}."
                )?;
            }
            if diff.primary_key_mismatch {
                writeln!(
                    f,
                    "  - Table `{table}`: primary key constraint differs between {live} and {expected}."
                )?;
            }
            if diff.foreign_key_mismatch {
                writeln!(
                    f,
                    "  - Table `{table}`: foreign key constraints differ between {live} and {expected}."
                )?;
            }
            for check in &diff.missing_checks {
                writeln!(
                    f,
                    "  - Table `{table}`: check `{check}` is in {expected} but missing from {live}."
                )?;
            }
            for check in &diff.extra_checks {
                writeln!(
                    f,
                    "  - Table `{table}`: check `{check}` exists in {live} but is missing from {expected}."
                )?;
            }
            for check in &diff.altered_checks {
                writeln!(
                    f,
                    "  - Table `{table}`: check `{check}` definition differs between {live} and {expected}."
                )?;
            }
            for index in &diff.missing_indexes {
                writeln!(
                    f,
                    "  - Table `{table}`: index `{index}` is in {expected} but missing from {live}."
                )?;
            }
            for index in &diff.extra_indexes {
                writeln!(
                    f,
                    "  - Table `{table}`: index `{index}` exists in {live} but is missing from {expected}."
                )?;
            }
            for index in &diff.altered_indexes {
                writeln!(
                    f,
                    "  - Table `{table}`: index `{index}` definition differs between {live} and {expected}."
                )?;
            }
        }
        Ok(())
    }
}

/// The wording a [`SchemaDiff`] is rendered with.
///
/// Both verifiers compare two catalogs, but the drift report names them after
/// `schema.lock` and the live database while the golden equation names them
/// after the declarative stage and the migration tail.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiffWording {
    /// How the catalog an item is expected in is named.
    expected: &'static str,
    /// How the catalog a finding is reported against is named.
    live: &'static str,
}

impl DiffWording {
    /// Wording of the drift verifier: `schema.lock` versus the live database.
    pub const SNAPSHOT: Self = Self {
        expected: "snapshot",
        live: "database",
    };

    /// Wording of the golden equation: the declarative stage versus the tail.
    pub const GOLDEN: Self = Self {
        expected: "the stage",
        live: "the tail",
    };
}

/// Discrepancies within a specific table.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct TableDiff {
    pub missing_columns: Vec<String>,
    pub extra_columns: Vec<String>,
    pub altered_columns: Vec<String>,
    pub primary_key_mismatch: bool,
    pub foreign_key_mismatch: bool,
    pub missing_checks: Vec<String>,
    pub extra_checks: Vec<String>,
    pub altered_checks: Vec<String>,
    pub missing_indexes: Vec<String>,
    pub extra_indexes: Vec<String>,
    pub altered_indexes: Vec<String>,
}

impl TableDiff {
    pub fn is_empty(&self) -> bool {
        self.missing_columns.is_empty()
            && self.extra_columns.is_empty()
            && self.altered_columns.is_empty()
            && !self.primary_key_mismatch
            && !self.foreign_key_mismatch
            && self.missing_checks.is_empty()
            && self.extra_checks.is_empty()
            && self.altered_checks.is_empty()
            && self.missing_indexes.is_empty()
            && self.extra_indexes.is_empty()
            && self.altered_indexes.is_empty()
    }
}

/// Actionable schema drift diagnostic report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DriftReport {
    pub expected_hash: String,
    pub live_hash: String,
    pub diff: SchemaDiff,
}

impl fmt::Display for DriftReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "Error: Schema drift detected!")?;
        writeln!(
            f,
            "Expected schema hash: {}\nLive schema hash:     {}",
            self.expected_hash, self.live_hash
        )?;
        self.diff.write_details(f, DiffWording::SNAPSHOT)?;
        writeln!(
            f,
            "\nHint: Run `cargo db-codegen` to update code and snapshot."
        )
    }
}

impl std::error::Error for DriftReport {}

/// Error encountered during drift verification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DriftError {
    Drift(Box<DriftReport>),
    HashMismatch {
        expected: String,
        found_in_code: String,
        /// Structural diff between the live catalog and the lock snapshot, so a
        /// stale `SCHEMA_HASH` still reports exactly what drifted.
        diff: Box<SchemaDiff>,
    },
    /// The generated schema code carries no `SCHEMA_HASH` constant.
    ///
    /// The constant is the only bridge between the generated code and the live
    /// catalog, so its absence would let a schema file that no codegen produced
    /// pass verification silently. The structural diff is carried along, so the
    /// report still names whatever else drifted.
    MissingSchemaHash {
        diff: Box<SchemaDiff>,
    },
    InvalidLock(String),
    Io(String),
}

impl fmt::Display for DriftError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DriftError::Drift(report) => write!(f, "{report}"),
            DriftError::HashMismatch {
                expected,
                found_in_code,
                diff,
            } => {
                writeln!(
                    f,
                    "SCHEMA_HASH mismatch in schema code: expected {expected}, found {found_in_code}"
                )?;
                diff.write_details(f, DiffWording::SNAPSHOT)?;
                write!(
                    f,
                    "\nHint: Run `cargo db-codegen` to update code and snapshot."
                )
            }
            DriftError::MissingSchemaHash { diff } => {
                writeln!(
                    f,
                    "The schema code declares no `SCHEMA_HASH` constant, so it cannot be \
                     verified against the live database."
                )?;
                diff.write_details(f, DiffWording::SNAPSHOT)?;
                write!(
                    f,
                    "\nHint: Run `cargo db-codegen` to regenerate the schema code and snapshot."
                )
            }
            DriftError::InvalidLock(message) => {
                write!(f, "invalid schema lock snapshot: {message}")
            }
            DriftError::Io(err) => write!(f, "I/O error during drift verification: {err}"),
        }
    }
}

impl std::error::Error for DriftError {}

/// Verifies that `live_catalog` bit-for-bit matches `expected_lock` and optional `schema_rs`.
pub fn verify_drift(
    live_catalog: &SchemaCatalog,
    expected_lock: &str,
    schema_rs: Option<&str>,
) -> Result<(), DriftError> {
    let live_hash = live_catalog.schema_hash();

    let expected_catalog = SchemaCatalog::from_lock(expected_lock)
        .map_err(|err| DriftError::InvalidLock(err.to_string()))?;
    let diff = compute_catalog_diff(live_catalog, &expected_catalog);

    if let Some(code) = schema_rs {
        match extract_schema_hash(code) {
            Some(embedded_hash) if embedded_hash == live_hash => {}
            Some(embedded_hash) => {
                return Err(DriftError::HashMismatch {
                    expected: live_hash,
                    found_in_code: embedded_hash,
                    diff: Box::new(diff),
                });
            }
            // A schema file without the constant cannot be tied to any catalog, so
            // it is rejected instead of passing the hash gate by omission.
            None => {
                return Err(DriftError::MissingSchemaHash {
                    diff: Box::new(diff),
                });
            }
        }
    }

    if diff.is_empty() {
        return Ok(());
    }

    // The hash identifies the canonical snapshot, so formatting differences of
    // the lock file (line endings, surrounding whitespace) never surface as a
    // drift between two equal catalogs.
    let expected_hash = expected_catalog.schema_hash();

    Err(DriftError::Drift(Box::new(DriftReport {
        expected_hash,
        live_hash,
        diff,
    })))
}

/// Extracts the `pub const SCHEMA_HASH: &str = "...";` literal from generated code.
///
/// Matching is anchored to the beginning of a line (ignoring leading
/// indentation) and skips single-line (`//`) and multi-line (`/* ... */`)
/// comments as well as string literals, so a commented-out or quoted constant
/// never yields a false positive.
pub fn extract_schema_hash(code: &str) -> Option<String> {
    const PREFIX: &str = "pub const SCHEMA_HASH: &str = \"";

    #[derive(Clone, Copy, PartialEq)]
    enum State {
        Code,
        LineComment,
        BlockComment,
        Str,
    }

    let mut state = State::Code;
    let mut block_depth: usize = 0;
    // True while only whitespace has been seen since the last newline, so the
    // next non-whitespace code character sits at the logical line start.
    let mut line_indent_only = true;

    let mut iter = code.char_indices().peekable();

    while let Some((index, ch)) = iter.next() {
        match state {
            State::Code => {
                if line_indent_only && ch == 'p' && code[index..].starts_with(PREFIX) {
                    let rest = &code[index + PREFIX.len()..];
                    let end = rest.find('"')?;
                    return Some(rest[..end].to_owned());
                }

                match ch {
                    '/' => match iter.peek() {
                        Some(&(_, '/')) => {
                            iter.next();
                            state = State::LineComment;
                            line_indent_only = false;
                        }
                        Some(&(_, '*')) => {
                            iter.next();
                            state = State::BlockComment;
                            block_depth = 1;
                            line_indent_only = false;
                        }
                        _ => line_indent_only = false,
                    },
                    '"' => {
                        state = State::Str;
                        line_indent_only = false;
                    }
                    '\n' => line_indent_only = true,
                    c if c.is_whitespace() => {}
                    _ => line_indent_only = false,
                }
            }
            State::LineComment => {
                if ch == '\n' {
                    state = State::Code;
                    line_indent_only = true;
                }
            }
            State::BlockComment => {
                if ch == '*' && matches!(iter.peek(), Some(&(_, '/'))) {
                    iter.next();
                    block_depth = block_depth.saturating_sub(1);
                    if block_depth == 0 {
                        state = State::Code;
                    }
                    line_indent_only = false;
                } else if ch == '/' && matches!(iter.peek(), Some(&(_, '*'))) {
                    iter.next();
                    block_depth += 1;
                } else if ch == '\n' {
                    line_indent_only = true;
                } else if !ch.is_whitespace() {
                    line_indent_only = false;
                }
            }
            State::Str => {
                if ch == '\\' {
                    iter.next();
                } else if ch == '"' {
                    state = State::Code;
                } else if ch == '\n' {
                    state = State::Code;
                    line_indent_only = true;
                }
            }
        }
    }

    None
}

/// Computes the structural differences between the `live` and `expected` catalogs.
pub(crate) fn compute_catalog_diff(live: &SchemaCatalog, expected: &SchemaCatalog) -> SchemaDiff {
    let mut diff = SchemaDiff::default();

    let live_enums: BTreeMap<&str, &EnumDef> =
        live.enums.iter().map(|e| (e.name.as_str(), e)).collect();
    let expected_enums: BTreeMap<&str, &EnumDef> = expected
        .enums
        .iter()
        .map(|e| (e.name.as_str(), e))
        .collect();

    let all_enums: BTreeSet<&str> = live_enums
        .keys()
        .copied()
        .chain(expected_enums.keys().copied())
        .collect();

    for enum_name in all_enums {
        match (live_enums.get(enum_name), expected_enums.get(enum_name)) {
            (None, Some(_)) => diff.missing_enums.push(enum_name.to_owned()),
            (Some(_), None) => diff.extra_enums.push(enum_name.to_owned()),
            (Some(live_enum), Some(expected_enum)) => {
                if live_enum.variants != expected_enum.variants {
                    diff.altered_enums.push(enum_name.to_owned());
                }
            }
            (None, None) => {}
        }
    }

    let live_views: BTreeMap<&str, &ViewDef> =
        live.views.iter().map(|v| (v.name.as_str(), v)).collect();
    let expected_views: BTreeMap<&str, &ViewDef> = expected
        .views
        .iter()
        .map(|v| (v.name.as_str(), v))
        .collect();

    let all_views: BTreeSet<&str> = live_views
        .keys()
        .copied()
        .chain(expected_views.keys().copied())
        .collect();

    for view_name in all_views {
        match (live_views.get(view_name), expected_views.get(view_name)) {
            (None, Some(_)) => diff.missing_views.push(view_name.to_owned()),
            (Some(_), None) => diff.extra_views.push(view_name.to_owned()),
            (Some(live_view), Some(expected_view)) => {
                if live_view.definition != expected_view.definition {
                    diff.altered_views.push(view_name.to_owned());
                }
            }
            (None, None) => {}
        }
    }

    let live_triggers: BTreeMap<&str, &TriggerDef> =
        live.triggers.iter().map(|t| (t.name.as_str(), t)).collect();
    let expected_triggers: BTreeMap<&str, &TriggerDef> = expected
        .triggers
        .iter()
        .map(|t| (t.name.as_str(), t))
        .collect();

    let all_triggers: BTreeSet<&str> = live_triggers
        .keys()
        .copied()
        .chain(expected_triggers.keys().copied())
        .collect();

    for trigger_name in all_triggers {
        match (
            live_triggers.get(trigger_name),
            expected_triggers.get(trigger_name),
        ) {
            (None, Some(_)) => diff.missing_triggers.push(trigger_name.to_owned()),
            (Some(_), None) => diff.extra_triggers.push(trigger_name.to_owned()),
            (Some(live_trigger), Some(expected_trigger)) => {
                if live_trigger.table_name != expected_trigger.table_name
                    || live_trigger.definition != expected_trigger.definition
                {
                    diff.altered_triggers.push(trigger_name.to_owned());
                }
            }
            (None, None) => {}
        }
    }

    let live_tables: BTreeMap<&str, &TableDef> =
        live.tables.iter().map(|t| (t.name.as_str(), t)).collect();
    let expected_tables: BTreeMap<&str, &TableDef> = expected
        .tables
        .iter()
        .map(|t| (t.name.as_str(), t))
        .collect();

    let all_tables: BTreeSet<&str> = live_tables
        .keys()
        .copied()
        .chain(expected_tables.keys().copied())
        .collect();

    for table_name in all_tables {
        match (live_tables.get(table_name), expected_tables.get(table_name)) {
            (None, Some(_)) => diff.missing_tables.push(table_name.to_owned()),
            (Some(_), None) => diff.extra_tables.push(table_name.to_owned()),
            (Some(live_table), Some(expected_table)) => {
                let table_diff = compute_table_diff(live_table, expected_table);
                if !table_diff.is_empty() {
                    diff.table_diffs.insert(table_name.to_owned(), table_diff);
                }
            }
            (None, None) => {}
        }
    }

    diff
}

/// Collapses internal whitespace runs and trims the ends of a check expression.
///
/// Introspection may report the SQL text of a check constraint with different
/// spacing than the snapshot records, so two constraints describing the same
/// predicate are compared only after their whitespace is normalised.
fn normalize_expression(expression: &str) -> String {
    expression.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn compute_table_diff(live: &TableDef, expected: &TableDef) -> TableDiff {
    let mut diff = TableDiff::default();

    let live_cols: BTreeMap<&str, &ColumnDef> =
        live.columns.iter().map(|c| (c.name.as_str(), c)).collect();
    let expected_cols: BTreeMap<&str, &ColumnDef> = expected
        .columns
        .iter()
        .map(|c| (c.name.as_str(), c))
        .collect();

    let all_cols: BTreeSet<&str> = live_cols
        .keys()
        .copied()
        .chain(expected_cols.keys().copied())
        .collect();

    for col in all_cols {
        match (live_cols.get(col), expected_cols.get(col)) {
            (None, Some(_)) => diff.missing_columns.push(col.to_owned()),
            (Some(_), None) => diff.extra_columns.push(col.to_owned()),
            (Some(live_col), Some(expected_col)) => {
                if live_col.type_kind != expected_col.type_kind
                    || live_col.nullable != expected_col.nullable
                    || live_col.default != expected_col.default
                {
                    diff.altered_columns.push(col.to_owned());
                }
            }
            (None, None) => {}
        }
    }

    if live.primary_key != expected.primary_key {
        diff.primary_key_mismatch = true;
    }

    let live_checks: BTreeMap<&str, &CheckDef> = live
        .checks
        .iter()
        .map(|check| (check.name.as_str(), check))
        .collect();
    let expected_checks: BTreeMap<&str, &CheckDef> = expected
        .checks
        .iter()
        .map(|check| (check.name.as_str(), check))
        .collect();

    let all_checks: BTreeSet<&str> = live_checks
        .keys()
        .copied()
        .chain(expected_checks.keys().copied())
        .collect();

    for check in all_checks {
        match (live_checks.get(check), expected_checks.get(check)) {
            (None, Some(_)) => diff.missing_checks.push(check.to_owned()),
            (Some(_), None) => diff.extra_checks.push(check.to_owned()),
            (Some(live_check), Some(expected_check)) => {
                if normalize_expression(&live_check.expression)
                    != normalize_expression(&expected_check.expression)
                {
                    diff.altered_checks.push(check.to_owned());
                }
            }
            (None, None) => {}
        }
    }

    let live_foreign_keys: BTreeMap<&str, &_> = live
        .foreign_keys
        .iter()
        .map(|fk| (fk.name.as_str(), fk))
        .collect();
    let expected_foreign_keys: BTreeMap<&str, &_> = expected
        .foreign_keys
        .iter()
        .map(|fk| (fk.name.as_str(), fk))
        .collect();
    if live_foreign_keys != expected_foreign_keys {
        diff.foreign_key_mismatch = true;
    }

    let live_indexes: BTreeMap<&str, &IndexDef> = live
        .indexes
        .iter()
        .map(|index| (index.name.as_str(), index))
        .collect();
    let expected_indexes: BTreeMap<&str, &IndexDef> = expected
        .indexes
        .iter()
        .map(|index| (index.name.as_str(), index))
        .collect();

    let all_indexes: BTreeSet<&str> = live_indexes
        .keys()
        .copied()
        .chain(expected_indexes.keys().copied())
        .collect();

    for index in all_indexes {
        match (live_indexes.get(index), expected_indexes.get(index)) {
            (None, Some(_)) => diff.missing_indexes.push(index.to_owned()),
            (Some(_), None) => diff.extra_indexes.push(index.to_owned()),
            (Some(live_index), Some(expected_index)) => {
                if live_index.unique != expected_index.unique
                    || live_index.columns != expected_index.columns
                    || live_index.method != expected_index.method
                    || live_index.predicate != expected_index.predicate
                    || live_index.expression != expected_index.expression
                {
                    diff.altered_indexes.push(index.to_owned());
                }
            }
            (None, None) => {}
        }
    }

    diff
}

#[cfg(test)]
mod tests {
    use super::*;
    use rurqe_migrate::catalog::{ForeignKeyAction, ForeignKeyDef, IndexDef, TypeKind};

    fn users_table(indexes: Vec<IndexDef>) -> TableDef {
        let mut table = TableDef::new(
            "users",
            vec![
                ColumnDef::new("id", TypeKind::Int64, false),
                ColumnDef::new("email", TypeKind::Text, false),
            ],
            vec!["id".to_owned()],
        );
        for index in indexes {
            table = table.with_index(index);
        }
        table
    }

    #[test]
    fn enum_drift_is_detected_and_reported() {
        let baseline =
            SchemaCatalog::new(vec![users_table(Vec::new())]).with_enums(vec![EnumDef::new(
                "user_role",
                vec!["admin".to_owned(), "member".to_owned()],
            )]);
        let lock = baseline.to_lock();

        // Extra enum in live database
        let live = SchemaCatalog::new(vec![users_table(Vec::new())]).with_enums(vec![
            EnumDef::new("user_role", vec!["admin".to_owned(), "member".to_owned()]),
            EnumDef::new("task_state", vec!["todo".to_owned(), "done".to_owned()]),
        ]);

        let err = verify_drift(&live, &lock, None).unwrap_err();
        match err {
            DriftError::Drift(report) => {
                assert_eq!(report.diff.extra_enums, vec!["task_state".to_owned()]);
                assert!(report.to_string().contains(
                    "Enum `task_state` exists in database but is missing from snapshot."
                ));
            }
            other => panic!("expected Drift, got {other:?}"),
        }

        // Altered variants in live database
        let live_altered =
            SchemaCatalog::new(vec![users_table(Vec::new())]).with_enums(vec![EnumDef::new(
                "user_role",
                vec!["admin".to_owned(), "member".to_owned(), "guest".to_owned()],
            )]);

        let err2 = verify_drift(&live_altered, &lock, None).unwrap_err();
        match err2 {
            DriftError::Drift(report) => {
                assert_eq!(report.diff.altered_enums, vec!["user_role".to_owned()]);
                assert!(report.to_string().contains(
                    "Enum `user_role` definition differs between database and snapshot."
                ));
            }
            other => panic!("expected Drift, got {other:?}"),
        }
    }

    #[test]
    fn identical_catalogs_produce_no_drift() {
        let catalog = SchemaCatalog::new(vec![users_table(vec![IndexDef::new(
            "users_email_idx",
            vec!["email".to_owned()],
            true,
        )])]);

        let lock = catalog.to_lock();
        let code = format!(
            "pub const SCHEMA_HASH: &str = \"{}\";",
            catalog.schema_hash()
        );

        assert!(verify_drift(&catalog, &lock, Some(&code)).is_ok());
    }

    #[test]
    fn schema_code_without_a_hash_constant_is_rejected() {
        let catalog = SchemaCatalog::new(vec![TableDef::new(
            "users",
            vec![ColumnDef::new("id", TypeKind::Int64, false)],
            vec!["id".to_owned()],
        )]);
        let lock = catalog.to_lock();
        let code = "// generated code without the marker\npub fn unrelated() {}";

        match verify_drift(&catalog, &lock, Some(code)).unwrap_err() {
            DriftError::MissingSchemaHash { diff } => {
                assert!(diff.is_empty(), "an equal catalog drifts in no way");
            }
            other => panic!("expected MissingSchemaHash, got {other:?}"),
        }
    }

    #[test]
    fn drift_detected_when_column_added_in_database() {
        let baseline_catalog = SchemaCatalog::new(vec![TableDef::new(
            "users",
            vec![ColumnDef::new("id", TypeKind::Int64, false)],
            vec!["id".to_owned()],
        )]);
        let baseline_lock = baseline_catalog.to_lock();

        let live_catalog = SchemaCatalog::new(vec![TableDef::new(
            "users",
            vec![
                ColumnDef::new("id", TypeKind::Int64, false),
                ColumnDef::new("phone_number", TypeKind::Text, true),
            ],
            vec!["id".to_owned()],
        )]);

        let err = verify_drift(&live_catalog, &baseline_lock, None).unwrap_err();
        match err {
            DriftError::Drift(report) => {
                let msg = report.to_string();
                assert!(msg.contains("Schema drift detected!"));
                assert!(msg.contains("phone_number"));
                assert!(msg.contains("cargo db-codegen"));
            }
            other => panic!("expected Drift, got {other:?}"),
        }
    }

    #[test]
    fn primary_key_drift_is_reported() {
        let baseline = SchemaCatalog::new(vec![TableDef::new(
            "users",
            vec![ColumnDef::new("id", TypeKind::Int64, false)],
            vec!["id".to_owned()],
        )]);
        let lock = baseline.to_lock();

        let live = SchemaCatalog::new(vec![TableDef::new(
            "users",
            vec![ColumnDef::new("id", TypeKind::Int64, false)],
            Vec::new(),
        )]);

        match verify_drift(&live, &lock, None).unwrap_err() {
            DriftError::Drift(report) => {
                assert!(
                    report.diff.table_diffs["users"].primary_key_mismatch,
                    "primary key drift must be detected"
                );
                assert!(report.to_string().contains("primary key"));
            }
            other => panic!("expected Drift, got {other:?}"),
        }
    }

    #[test]
    fn foreign_key_drift_is_reported() {
        let with_fk = TableDef::new(
            "orders",
            vec![ColumnDef::new("user_id", TypeKind::Int64, false)],
            Vec::new(),
        )
        .with_foreign_key(ForeignKeyDef::new(
            "orders_user_id_fkey",
            vec!["user_id".to_owned()],
            "users",
            vec!["id".to_owned()],
        ));
        let lock = SchemaCatalog::new(vec![with_fk]).to_lock();

        let live = SchemaCatalog::new(vec![TableDef::new(
            "orders",
            vec![ColumnDef::new("user_id", TypeKind::Int64, false)],
            Vec::new(),
        )]);

        match verify_drift(&live, &lock, None).unwrap_err() {
            DriftError::Drift(report) => {
                assert!(
                    report.diff.table_diffs["orders"].foreign_key_mismatch,
                    "foreign key drift must be detected"
                );
            }
            other => panic!("expected Drift, got {other:?}"),
        }
    }

    #[test]
    fn foreign_key_column_changes_are_detected_structurally() {
        let snapshot = SchemaCatalog::new(vec![TableDef::new(
            "orders",
            vec![
                ColumnDef::new("user_id", TypeKind::Int64, false),
                ColumnDef::new("tenant_id", TypeKind::Int64, false),
            ],
            Vec::new(),
        )
        .with_foreign_key(ForeignKeyDef::new(
            "orders_user_fkey",
            vec!["user_id".to_owned()],
            "users",
            vec!["id".to_owned()],
        ))]);
        let lock = snapshot.to_lock();

        let live = SchemaCatalog::new(vec![TableDef::new(
            "orders",
            vec![
                ColumnDef::new("user_id", TypeKind::Int64, false),
                ColumnDef::new("tenant_id", TypeKind::Int64, false),
            ],
            Vec::new(),
        )
        .with_foreign_key(ForeignKeyDef::new(
            "orders_user_fkey",
            vec!["user_id".to_owned(), "tenant_id".to_owned()],
            "users",
            vec!["id".to_owned(), "tenant_id".to_owned()],
        ))]);

        match verify_drift(&live, &lock, None).unwrap_err() {
            DriftError::Drift(report) => {
                assert!(
                    report.diff.table_diffs["orders"].foreign_key_mismatch,
                    "column-level foreign key drift must be detected"
                );
            }
            other => panic!("expected Drift, got {other:?}"),
        }
    }

    #[test]
    fn missing_indexes_are_reported() {
        let snapshot = SchemaCatalog::new(vec![users_table(vec![IndexDef::new(
            "users_email_idx",
            vec!["email".to_owned()],
            true,
        )])]);
        let lock = snapshot.to_lock();

        let live = SchemaCatalog::new(vec![users_table(Vec::new())]);

        match verify_drift(&live, &lock, None).unwrap_err() {
            DriftError::Drift(report) => {
                assert_eq!(
                    report.diff.table_diffs["users"].missing_indexes,
                    vec!["users_email_idx".to_owned()]
                );
                assert!(report.to_string().contains("users_email_idx"));
            }
            other => panic!("expected Drift, got {other:?}"),
        }
    }

    #[test]
    fn extra_indexes_are_reported() {
        let snapshot = SchemaCatalog::new(vec![users_table(Vec::new())]);
        let lock = snapshot.to_lock();

        let live = SchemaCatalog::new(vec![users_table(vec![IndexDef::new(
            "users_email_idx",
            vec!["email".to_owned()],
            false,
        )])]);

        match verify_drift(&live, &lock, None).unwrap_err() {
            DriftError::Drift(report) => {
                assert_eq!(
                    report.diff.table_diffs["users"].extra_indexes,
                    vec!["users_email_idx".to_owned()]
                );
            }
            other => panic!("expected Drift, got {other:?}"),
        }
    }

    #[test]
    fn index_uniqueness_changes_are_reported() {
        let snapshot = SchemaCatalog::new(vec![users_table(vec![IndexDef::new(
            "users_email_idx",
            vec!["email".to_owned()],
            true,
        )])]);
        let lock = snapshot.to_lock();

        let live = SchemaCatalog::new(vec![users_table(vec![IndexDef::new(
            "users_email_idx",
            vec!["email".to_owned()],
            false,
        )])]);

        match verify_drift(&live, &lock, None).unwrap_err() {
            DriftError::Drift(report) => {
                assert_eq!(
                    report.diff.table_diffs["users"].altered_indexes,
                    vec!["users_email_idx".to_owned()]
                );
                assert!(report.to_string().contains("index"));
            }
            other => panic!("expected Drift, got {other:?}"),
        }
    }

    #[test]
    fn index_column_changes_are_reported() {
        let snapshot = SchemaCatalog::new(vec![users_table(vec![IndexDef::new(
            "users_email_idx",
            vec!["email".to_owned()],
            false,
        )])]);
        let lock = snapshot.to_lock();

        let live = SchemaCatalog::new(vec![users_table(vec![IndexDef::new(
            "users_email_idx",
            vec!["email".to_owned(), "id".to_owned()],
            false,
        )])]);

        match verify_drift(&live, &lock, None).unwrap_err() {
            DriftError::Drift(report) => {
                assert_eq!(
                    report.diff.table_diffs["users"].altered_indexes,
                    vec!["users_email_idx".to_owned()]
                );
            }
            other => panic!("expected Drift, got {other:?}"),
        }
    }

    #[test]
    fn altered_column_definition_is_reported() {
        let baseline = SchemaCatalog::new(vec![TableDef::new(
            "users",
            vec![ColumnDef::new("email", TypeKind::Text, false)],
            Vec::new(),
        )]);
        let lock = baseline.to_lock();

        let live = SchemaCatalog::new(vec![TableDef::new(
            "users",
            vec![ColumnDef::new("email", TypeKind::Text, true)],
            Vec::new(),
        )]);

        match verify_drift(&live, &lock, None).unwrap_err() {
            DriftError::Drift(report) => {
                assert_eq!(
                    report.diff.table_diffs["users"].altered_columns,
                    vec!["email".to_owned()]
                );
            }
            other => panic!("expected Drift, got {other:?}"),
        }
    }

    #[test]
    fn escaped_identifiers_are_matched_without_false_drift() {
        let catalog = SchemaCatalog::new(vec![TableDef::new(
            "we\"ird",
            vec![ColumnDef::new("sel\"ect", TypeKind::Text, false)],
            vec!["sel\"ect".to_owned()],
        )]);
        let lock = catalog.to_lock();

        assert!(verify_drift(&catalog, &lock, None).is_ok());
    }

    #[test]
    fn malformed_lock_reports_invalid_lock() {
        let catalog = SchemaCatalog::new(vec![TableDef::new(
            "users",
            vec![ColumnDef::new("id", TypeKind::Int64, false)],
            vec!["id".to_owned()],
        )]);

        match verify_drift(&catalog, "not a schema lock", None).unwrap_err() {
            DriftError::InvalidLock(_) => {}
            other => panic!("expected InvalidLock, got {other:?}"),
        }
    }

    #[test]
    fn hash_extraction_ignores_line_comments() {
        assert_eq!(
            extract_schema_hash("// pub const SCHEMA_HASH: &str = \"dead\";"),
            None
        );
    }

    #[test]
    fn hash_extraction_ignores_block_comments() {
        let code = "/*\npub const SCHEMA_HASH: &str = \"dead\";\n*/";
        assert_eq!(extract_schema_hash(code), None);
    }

    #[test]
    fn hash_extraction_ignores_strings() {
        let code = r#"let s = "pub const SCHEMA_HASH: &str = \"dead\"";"#;
        assert_eq!(extract_schema_hash(code), None);
    }

    #[test]
    fn hash_extraction_anchors_to_line_start() {
        let code = "let marker = \"x\"; pub const SCHEMA_HASH: &str = \"dead\";";
        assert_eq!(extract_schema_hash(code), None);
    }

    #[test]
    fn hash_extraction_reads_indented_constant() {
        let code = "mod schema {\n    pub const SCHEMA_HASH: &str = \"abc123\";\n}";
        assert_eq!(extract_schema_hash(code), Some("abc123".to_owned()));
    }

    #[test]
    fn hash_extraction_prefers_real_constant_after_comment() {
        let code =
            "// pub const SCHEMA_HASH: &str = \"dead\";\npub const SCHEMA_HASH: &str = \"live\";";
        assert_eq!(extract_schema_hash(code), Some("live".to_owned()));
    }

    #[test]
    fn view_drift_is_reported() {
        let baseline = SchemaCatalog::new(vec![users_table(Vec::new())])
            .with_views(vec![ViewDef::new("active_users", "SELECT id FROM users")]);
        let lock = baseline.to_lock();

        let live = SchemaCatalog::new(vec![users_table(Vec::new())]);
        match verify_drift(&live, &lock, None).unwrap_err() {
            DriftError::Drift(report) => {
                assert_eq!(report.diff.missing_views, vec!["active_users".to_owned()]);
                assert!(report.to_string().contains("View `active_users`"));
            }
            other => panic!("expected Drift, got {other:?}"),
        }

        let live_extra = SchemaCatalog::new(vec![users_table(Vec::new())]).with_views(vec![
            ViewDef::new("active_users", "SELECT id FROM users"),
            ViewDef::new("archived_users", "SELECT id FROM users"),
        ]);
        match verify_drift(&live_extra, &lock, None).unwrap_err() {
            DriftError::Drift(report) => {
                assert_eq!(report.diff.extra_views, vec!["archived_users".to_owned()]);
            }
            other => panic!("expected Drift, got {other:?}"),
        }

        let live_altered =
            SchemaCatalog::new(vec![users_table(Vec::new())]).with_views(vec![ViewDef::new(
                "active_users",
                "SELECT id, email FROM users",
            )]);
        match verify_drift(&live_altered, &lock, None).unwrap_err() {
            DriftError::Drift(report) => {
                assert_eq!(report.diff.altered_views, vec!["active_users".to_owned()]);
                assert!(report
                    .to_string()
                    .contains("View `active_users` definition differs"));
            }
            other => panic!("expected Drift, got {other:?}"),
        }
    }

    #[test]
    fn trigger_drift_is_reported() {
        let baseline = SchemaCatalog::new(vec![users_table(Vec::new())])
            .with_triggers(vec![TriggerDef::new("users_audit", "users", "BEGIN; END;")]);
        let lock = baseline.to_lock();

        let live = SchemaCatalog::new(vec![users_table(Vec::new())]);
        match verify_drift(&live, &lock, None).unwrap_err() {
            DriftError::Drift(report) => {
                assert_eq!(report.diff.missing_triggers, vec!["users_audit".to_owned()]);
                assert!(report.to_string().contains("Trigger `users_audit`"));
            }
            other => panic!("expected Drift, got {other:?}"),
        }

        let live_extra = SchemaCatalog::new(vec![users_table(Vec::new())]).with_triggers(vec![
            TriggerDef::new("users_audit", "users", "BEGIN; END;"),
            TriggerDef::new("users_touch", "users", "BEGIN; END;"),
        ]);
        match verify_drift(&live_extra, &lock, None).unwrap_err() {
            DriftError::Drift(report) => {
                assert_eq!(report.diff.extra_triggers, vec!["users_touch".to_owned()]);
            }
            other => panic!("expected Drift, got {other:?}"),
        }

        let live_altered =
            SchemaCatalog::new(vec![users_table(Vec::new())]).with_triggers(vec![TriggerDef::new(
                "users_audit",
                "users",
                "BEGIN; UPDATE x; END;",
            )]);
        match verify_drift(&live_altered, &lock, None).unwrap_err() {
            DriftError::Drift(report) => {
                assert_eq!(report.diff.altered_triggers, vec!["users_audit".to_owned()]);
            }
            other => panic!("expected Drift, got {other:?}"),
        }
    }

    #[test]
    fn check_constraint_drift_is_reported() {
        let baseline = SchemaCatalog::new(vec![
            users_table(Vec::new()).with_check(CheckDef::new("users_age_check", "age >= 0"))
        ]);
        let lock = baseline.to_lock();

        let live = SchemaCatalog::new(vec![users_table(Vec::new())]);
        match verify_drift(&live, &lock, None).unwrap_err() {
            DriftError::Drift(report) => {
                assert_eq!(
                    report.diff.table_diffs["users"].missing_checks,
                    vec!["users_age_check".to_owned()]
                );
                assert!(report.to_string().contains("users_age_check"));
            }
            other => panic!("expected Drift, got {other:?}"),
        }

        let live_extra = SchemaCatalog::new(vec![users_table(Vec::new())
            .with_check(CheckDef::new("users_age_check", "age >= 0"))
            .with_check(CheckDef::new("users_name_check", "length(email) > 0"))]);
        match verify_drift(&live_extra, &lock, None).unwrap_err() {
            DriftError::Drift(report) => {
                assert_eq!(
                    report.diff.table_diffs["users"].extra_checks,
                    vec!["users_name_check".to_owned()]
                );
            }
            other => panic!("expected Drift, got {other:?}"),
        }

        let live_altered = SchemaCatalog::new(vec![
            users_table(Vec::new()).with_check(CheckDef::new("users_age_check", "age > 0"))
        ]);
        match verify_drift(&live_altered, &lock, None).unwrap_err() {
            DriftError::Drift(report) => {
                assert_eq!(
                    report.diff.table_diffs["users"].altered_checks,
                    vec!["users_age_check".to_owned()]
                );
            }
            other => panic!("expected Drift, got {other:?}"),
        }
    }

    #[test]
    fn whitespace_differences_in_check_expressions_are_ignored() {
        let baseline = SchemaCatalog::new(vec![
            users_table(Vec::new()).with_check(CheckDef::new("users_age_check", "age >= 0"))
        ]);
        let lock = baseline.to_lock();

        let live =
            SchemaCatalog::new(vec![users_table(Vec::new())
                .with_check(CheckDef::new("users_age_check", "age   >=    0"))]);
        assert!(verify_drift(&live, &lock, None).is_ok());
    }

    #[test]
    fn index_attribute_drift_is_reported() {
        let baseline = SchemaCatalog::new(vec![users_table(vec![IndexDef::new(
            "users_email_idx",
            vec!["email".to_owned()],
            false,
        )
        .with_method("btree")
        .with_predicate("email IS NOT NULL")])]);
        let lock = baseline.to_lock();

        let live = SchemaCatalog::new(vec![users_table(vec![IndexDef::new(
            "users_email_idx",
            vec!["email".to_owned()],
            false,
        )
        .with_method("hash")])]);

        match verify_drift(&live, &lock, None).unwrap_err() {
            DriftError::Drift(report) => {
                assert_eq!(
                    report.diff.table_diffs["users"].altered_indexes,
                    vec!["users_email_idx".to_owned()]
                );
                assert!(report.to_string().contains("users_email_idx"));
            }
            other => panic!("expected Drift, got {other:?}"),
        }
    }

    #[test]
    fn foreign_key_action_drift_is_reported() {
        let baseline = SchemaCatalog::new(vec![TableDef::new(
            "orders",
            vec![ColumnDef::new("user_id", TypeKind::Int64, false)],
            Vec::new(),
        )
        .with_foreign_key(
            ForeignKeyDef::new(
                "orders_user_id_fkey",
                vec!["user_id".to_owned()],
                "users",
                vec!["id".to_owned()],
            )
            .with_actions(ForeignKeyAction::Cascade, ForeignKeyAction::NoAction),
        )]);
        let lock = baseline.to_lock();

        let live = SchemaCatalog::new(vec![TableDef::new(
            "orders",
            vec![ColumnDef::new("user_id", TypeKind::Int64, false)],
            Vec::new(),
        )
        .with_foreign_key(
            ForeignKeyDef::new(
                "orders_user_id_fkey",
                vec!["user_id".to_owned()],
                "users",
                vec!["id".to_owned()],
            )
            .with_actions(ForeignKeyAction::Restrict, ForeignKeyAction::NoAction),
        )]);

        match verify_drift(&live, &lock, None).unwrap_err() {
            DriftError::Drift(report) => {
                assert!(
                    report.diff.table_diffs["orders"].foreign_key_mismatch,
                    "an action change must surface as a foreign key mismatch"
                );
            }
            other => panic!("expected Drift, got {other:?}"),
        }
    }
}
