//! Deterministic schema drift detection against `schema.lock` snapshots.
//!
//! Compares a live introspected database catalog against the canonical `schema.lock`
//! file and `pub const SCHEMA_HASH` embedded in generated Rust code. Emits
//! actionable, fail-fast diagnostic error reports when code and database diverge.

use core::fmt;
use std::collections::{BTreeMap, BTreeSet};

use rurqe_migrate::catalog::{ColumnDef, SchemaCatalog, TableDef};

/// Structural schema diff describing exact discrepancies.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SchemaDiff {
    pub missing_tables: Vec<String>,
    pub extra_tables: Vec<String>,
    pub table_diffs: BTreeMap<String, TableDiff>,
}

impl SchemaDiff {
    /// Whether any differences were detected.
    pub fn is_empty(&self) -> bool {
        self.missing_tables.is_empty()
            && self.extra_tables.is_empty()
            && self.table_diffs.is_empty()
    }

    /// Renders every discrepancy as an indented bullet, one line per finding.
    fn write_details(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for table in &self.missing_tables {
            writeln!(
                f,
                "  - Table `{table}` is declared in snapshot but missing from live database."
            )?;
        }
        for table in &self.extra_tables {
            writeln!(
                f,
                "  - Table `{table}` exists in live database but is missing from snapshot."
            )?;
        }
        for (table, diff) in &self.table_diffs {
            for col in &diff.missing_columns {
                writeln!(
                    f,
                    "  - Table `{table}`: column `{col}` is in snapshot but missing from database."
                )?;
            }
            for col in &diff.extra_columns {
                writeln!(
                    f,
                    "  - Table `{table}`: column `{col}` exists in database but is missing from snapshot."
                )?;
            }
            for col in &diff.altered_columns {
                writeln!(
                    f,
                    "  - Table `{table}`: column `{col}` definition differs between database and snapshot."
                )?;
            }
            if diff.primary_key_mismatch {
                writeln!(
                    f,
                    "  - Table `{table}`: primary key constraint differs between database and snapshot."
                )?;
            }
            if diff.foreign_key_mismatch {
                writeln!(
                    f,
                    "  - Table `{table}`: foreign key constraints differ between database and snapshot."
                )?;
            }
        }
        Ok(())
    }
}

/// Discrepancies within a specific table.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct TableDiff {
    pub missing_columns: Vec<String>,
    pub extra_columns: Vec<String>,
    pub altered_columns: Vec<String>,
    pub primary_key_mismatch: bool,
    pub foreign_key_mismatch: bool,
}

impl TableDiff {
    pub fn is_empty(&self) -> bool {
        self.missing_columns.is_empty()
            && self.extra_columns.is_empty()
            && self.altered_columns.is_empty()
            && !self.primary_key_mismatch
            && !self.foreign_key_mismatch
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
        self.diff.write_details(f)?;
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
    Drift(DriftReport),
    HashMismatch {
        expected: String,
        found_in_code: String,
        /// Structural diff between the live catalog and the lock snapshot, so a
        /// stale `SCHEMA_HASH` still reports exactly what drifted.
        diff: SchemaDiff,
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
                diff.write_details(f)?;
                write!(
                    f,
                    "\nHint: Run `cargo db-codegen` to update code and snapshot."
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
        if let Some(embedded_hash) = extract_schema_hash(code) {
            if embedded_hash != live_hash {
                return Err(DriftError::HashMismatch {
                    expected: live_hash,
                    found_in_code: embedded_hash,
                    diff,
                });
            }
        }
    }

    if diff.is_empty() {
        return Ok(());
    }

    let expected_hash = rurqe_migrate::checksum::Checksum::of(expected_lock.as_bytes()).to_hex();

    Err(DriftError::Drift(DriftReport {
        expected_hash,
        live_hash,
        diff,
    }))
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

    diff
}

#[cfg(test)]
mod tests {
    use super::*;
    use rurqe_migrate::catalog::{ForeignKeyDef, TypeKind};

    #[test]
    fn identical_catalogs_produce_no_drift() {
        let catalog = SchemaCatalog::new(vec![TableDef::new(
            "users",
            vec![
                ColumnDef::new("id", TypeKind::Int64, false),
                ColumnDef::new("email", TypeKind::Text, false),
            ],
            vec!["id".to_owned()],
        )]);

        let lock = catalog.to_lock();
        let code = format!("pub const SCHEMA_HASH: &str = \"{}\";", catalog.schema_hash());

        assert!(verify_drift(&catalog, &lock, Some(&code)).is_ok());
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
        let code = "// pub const SCHEMA_HASH: &str = \"dead\";\npub const SCHEMA_HASH: &str = \"live\";";
        assert_eq!(extract_schema_hash(code), Some("live".to_owned()));
    }
}
