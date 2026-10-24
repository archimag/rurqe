//! Structural comparison of two schema catalogs.
//!
//! The comparison is engine-neutral: it walks the enums, views, triggers, tables,
//! columns, keys, checks, and indexes of two [`SchemaCatalog`] values and records
//! every discrepancy in a [`SchemaDiff`]. Both verifiers of the toolchain — the
//! drift gate and the golden equation — reuse it, so one finding is described by
//! exactly one implementation.

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
