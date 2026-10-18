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
}

/// Discrepancies within a specific table.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct TableDiff {
    pub missing_columns: Vec<String>,
    pub extra_columns: Vec<String>,
    pub altered_columns: Vec<String>,
    pub primary_key_mismatch: bool,
}

impl TableDiff {
    pub fn is_empty(&self) -> bool {
        self.missing_columns.is_empty()
            && self.extra_columns.is_empty()
            && self.altered_columns.is_empty()
            && !self.primary_key_mismatch
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

        for table in &self.diff.missing_tables {
            writeln!(
                f,
                "  - Table `{table}` is declared in snapshot but missing from live database."
            )?;
        }
        for table in &self.diff.extra_tables {
            writeln!(
                f,
                "  - Table `{table}` exists in live database but is missing from snapshot."
            )?;
        }
        for (table, diff) in &self.diff.table_diffs {
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
        }

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
    },
    Io(String),
}

impl fmt::Display for DriftError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DriftError::Drift(report) => write!(f, "{report}"),
            DriftError::HashMismatch {
                expected,
                found_in_code,
            } => write!(
                f,
                "SCHEMA_HASH mismatch in schema code: expected {expected}, found {found_in_code}"
            ),
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
    let live_lock = live_catalog.to_lock();
    let live_hash = live_catalog.schema_hash();

    if let Some(code) = schema_rs {
        if let Some(embedded_hash) = extract_schema_hash(code) {
            if embedded_hash != live_hash {
                return Err(DriftError::HashMismatch {
                    expected: live_hash.clone(),
                    found_in_code: embedded_hash,
                });
            }
        }
    }

    if live_lock.trim() == expected_lock.trim() {
        return Ok(());
    }

    let diff = compute_catalog_diff(live_catalog, expected_lock);
    let expected_hash = rurqe_migrate::checksum::Checksum::of(expected_lock.as_bytes()).to_hex();

    Err(DriftError::Drift(DriftReport {
        expected_hash,
        live_hash,
        diff,
    }))
}

/// Extracts the `pub const SCHEMA_HASH: &str = "...";` literal from generated code.
pub fn extract_schema_hash(code: &str) -> Option<String> {
    const PREFIX: &str = "pub const SCHEMA_HASH: &str = \"";
    let start = code.find(PREFIX)?;
    let rest = &code[start + PREFIX.len()..];
    let end = rest.find('"')?;
    Some(rest[..end].to_owned())
}

fn compute_catalog_diff(live: &SchemaCatalog, lock_text: &str) -> SchemaDiff {
    let mut diff = SchemaDiff::default();

    let live_tables: BTreeMap<&str, &TableDef> =
        live.tables.iter().map(|t| (t.name.as_str(), t)).collect();

    let expected_table_names = parse_table_names_from_lock(lock_text);

    let all_tables: BTreeSet<&str> = live_tables
        .keys()
        .copied()
        .chain(expected_table_names.iter().map(|s| s.as_str()))
        .collect();

    for table_name in all_tables {
        let in_live = live_tables.get(table_name);
        let in_expected = expected_table_names.contains(table_name);

        match (in_live, in_expected) {
            (None, true) => diff.missing_tables.push(table_name.to_owned()),
            (Some(_), false) => diff.extra_tables.push(table_name.to_owned()),
            (Some(live_table), true) => {
                let table_diff = compute_table_diff(live_table, lock_text);
                if !table_diff.is_empty() {
                    diff.table_diffs.insert(table_name.to_owned(), table_diff);
                }
            }
            (None, false) => {}
        }
    }

    diff
}

fn parse_table_names_from_lock(lock: &str) -> BTreeSet<String> {
    let mut set = BTreeSet::new();
    for line in lock.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("table \"") {
            if let Some(inner) = rest.strip_suffix("\" {") {
                set.insert(inner.replace("\"\"", "\""));
            }
        }
    }
    set
}

fn compute_table_diff(live: &TableDef, lock_text: &str) -> TableDiff {
    let mut diff = TableDiff::default();

    let expected_columns = parse_columns_from_lock(lock_text, &live.name);
    let live_cols: BTreeMap<&str, &ColumnDef> =
        live.columns.iter().map(|c| (c.name.as_str(), c)).collect();

    let all_cols: BTreeSet<&str> = live_cols
        .keys()
        .copied()
        .chain(expected_columns.keys().copied())
        .collect();

    for col in all_cols {
        let in_live = live_cols.get(col);
        let in_expected = expected_columns.get(col);

        match (in_live, in_expected) {
            (None, Some(_)) => diff.missing_columns.push(col.to_owned()),
            (Some(_), None) => diff.extra_columns.push(col.to_owned()),
            (Some(live_c), Some(exp_str)) => {
                let live_str = format!(
                    "{}: {}{}",
                    live_c.name,
                    live_c.type_kind.as_str(),
                    if live_c.nullable { " null" } else { " not null" }
                );
                if !exp_str.contains(&live_str) {
                    diff.altered_columns.push(col.to_owned());
                }
            }
            (None, None) => {}
        }
    }

    diff
}

fn parse_columns_from_lock<'a>(lock: &'a str, table_name: &str) -> BTreeMap<&'a str, &'a str> {
    let mut map = BTreeMap::new();
    let needle = format!("table \"{table_name}\" {{");
    let mut in_table = false;

    for line in lock.lines() {
        let trimmed = line.trim();
        if trimmed == needle {
            in_table = true;
            continue;
        }
        if in_table {
            if trimmed == "}" {
                break;
            }
            if let Some(rest) = trimmed.strip_prefix("column \"") {
                if let Some(end_quote) = rest.find('"') {
                    let col_name = &rest[..end_quote];
                    map.insert(col_name, trimmed);
                }
            }
        }
    }

    map
}

#[cfg(test)]
mod tests {
    use super::*;
    use rurqe_migrate::catalog::{ColumnDef, TableDef, TypeKind};

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
}
