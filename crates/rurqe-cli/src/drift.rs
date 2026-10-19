//! Deterministic schema drift detection against `schema.lock` snapshots.
//!
//! Compares a live introspected database catalog against the canonical `schema.lock`
//! file and `pub const SCHEMA_HASH` embedded in generated Rust code. Emits
//! actionable, fail-fast diagnostic error reports when code and database diverge.

use core::fmt;
use std::collections::{BTreeMap, BTreeSet};

use rurqe_migrate::catalog::{ColumnDef, ForeignKeyDef, SchemaCatalog, TableDef};

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
            if diff.foreign_key_mismatch {
                writeln!(
                    f,
                    "  - Table `{table}`: foreign key constraints differ between database and snapshot."
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

    let expected_tables = parse_tables_from_lock(lock_text);
    let live_tables: BTreeMap<&str, &TableDef> =
        live.tables.iter().map(|t| (t.name.as_str(), t)).collect();

    let all_tables: BTreeSet<&str> = live_tables
        .keys()
        .copied()
        .chain(expected_tables.keys().map(|s| s.as_str()))
        .collect();

    for table_name in all_tables {
        let in_live = live_tables.get(table_name);
        let in_expected = expected_tables.get(table_name);

        match (in_live, in_expected) {
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

/// Structurally parsed representation of a `schema.lock` table block.
#[derive(Debug, Default)]
struct ExpectedTable {
    columns: BTreeMap<String, ExpectedColumn>,
    primary_key: Vec<String>,
    foreign_keys: BTreeSet<String>,
}

/// Structurally parsed representation of a `schema.lock` column line.
#[derive(Debug, PartialEq, Eq)]
struct ExpectedColumn {
    type_str: String,
    nullable: bool,
    default: Option<String>,
}

/// Parses every `table "..." { ... }` block of the snapshot.
fn parse_tables_from_lock(lock: &str) -> BTreeMap<String, ExpectedTable> {
    let mut tables = BTreeMap::new();
    let mut current: Option<(String, ExpectedTable)> = None;

    for line in lock.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("table ") {
            if let Some((name, _)) = parse_quoted(rest) {
                current = Some((name, ExpectedTable::default()));
            }
            continue;
        }
        if trimmed == "}" {
            if let Some((name, table)) = current.take() {
                tables.insert(name, table);
            }
            continue;
        }
        if let Some((_, table)) = current.as_mut() {
            parse_table_line(trimmed, table);
        }
    }

    tables
}

fn parse_table_line(line: &str, table: &mut ExpectedTable) {
    if let Some(rest) = line.strip_prefix("column ") {
        if let Some((name, after)) = parse_quoted(rest) {
            if let Some(definition) = after.strip_prefix(": ").and_then(parse_column_definition) {
                table.columns.insert(name, definition);
            }
        }
    } else if let Some(rest) = line.strip_prefix("primary key ") {
        if let Some(columns) = parse_quoted_list(rest) {
            table.primary_key = columns;
        }
    } else if line.starts_with("foreign key ") {
        table.foreign_keys.insert(line.to_owned());
    }
}

fn parse_column_definition(definition: &str) -> Option<ExpectedColumn> {
    let (head, default) = match definition.find(" default ") {
        Some(position) => (
            &definition[..position],
            Some(definition[position + " default ".len()..].to_owned()),
        ),
        None => (definition, None),
    };

    let (type_str, nullable) = if let Some(ty) = head.strip_suffix(" not null") {
        (ty, false)
    } else if let Some(ty) = head.strip_suffix(" null") {
        (ty, true)
    } else {
        (head, false)
    };

    Some(ExpectedColumn {
        type_str: type_str.to_owned(),
        nullable,
        default,
    })
}

fn compute_table_diff(live: &TableDef, expected: &ExpectedTable) -> TableDiff {
    let mut diff = TableDiff::default();

    let live_cols: BTreeMap<&str, &ColumnDef> =
        live.columns.iter().map(|c| (c.name.as_str(), c)).collect();

    let all_cols: BTreeSet<&str> = live_cols
        .keys()
        .copied()
        .chain(expected.columns.keys().map(|s| s.as_str()))
        .collect();

    for col in all_cols {
        match (live_cols.get(col), expected.columns.get(col)) {
            (None, Some(_)) => diff.missing_columns.push(col.to_owned()),
            (Some(_), None) => diff.extra_columns.push(col.to_owned()),
            (Some(live_col), Some(expected_col)) => {
                if live_col.type_kind.as_str() != expected_col.type_str
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

    let live_foreign_keys: BTreeSet<String> =
        live.foreign_keys.iter().map(render_foreign_key).collect();
    if live_foreign_keys != expected.foreign_keys {
        diff.foreign_key_mismatch = true;
    }

    diff
}

/// Renders a foreign key exactly as the canonical `schema.lock` snapshot does.
fn render_foreign_key(foreign_key: &ForeignKeyDef) -> String {
    format!(
        "foreign key {} {} references {} {}",
        quote_ident(&foreign_key.name),
        quote_list(&foreign_key.columns),
        quote_ident(&foreign_key.referenced_table),
        quote_list(&foreign_key.referenced_columns),
    )
}

fn quote_ident(ident: &str) -> String {
    let mut out = String::from("\"");
    for character in ident.chars() {
        if character == '"' {
            out.push('"');
        }
        out.push(character);
    }
    out.push('"');
    out
}

fn quote_list(idents: &[String]) -> String {
    let rendered: Vec<String> = idents.iter().map(|ident| quote_ident(ident)).collect();
    format!("({})", rendered.join(", "))
}

/// Parses a double-quoted identifier, unescaping doubled inner quotes.
fn parse_quoted(input: &str) -> Option<(String, &str)> {
    let mut chars = input.char_indices();
    let (_, first) = chars.next()?;
    if first != '"' {
        return None;
    }

    let mut name = String::new();
    let mut iter = chars;
    while let Some((index, character)) = iter.next() {
        if character == '"' {
            match iter.next() {
                Some((_, '"')) => name.push('"'),
                _ => return Some((name, &input[index + 1..])),
            }
        } else {
            name.push(character);
        }
    }

    None
}

/// Parses a parenthesised, comma-separated list of quoted identifiers.
fn parse_quoted_list(input: &str) -> Option<Vec<String>> {
    let inner = input.strip_prefix('(')?.strip_suffix(')')?;
    let mut idents = Vec::new();
    let mut cursor = inner;

    loop {
        let trimmed = cursor.trim_start();
        if trimmed.is_empty() {
            break;
        }
        let (ident, after) = parse_quoted(trimmed)?;
        idents.push(ident);
        let after = after.trim_start();
        match after.strip_prefix(',') {
            Some(next) => cursor = next,
            None => break,
        }
    }

    Some(idents)
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
        use rurqe_migrate::catalog::ForeignKeyDef;

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
}
