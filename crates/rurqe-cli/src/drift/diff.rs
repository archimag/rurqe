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
    CheckDef, ColumnDef, EnumDef, ForeignKeyDef, IndexDef, SchemaCatalog, TableDef, TriggerDef,
    ViewDef,
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
        write_findings(
            f,
            Category::Enum,
            None,
            Findings::of(&self.missing_enums, &self.extra_enums, &self.altered_enums),
            wording,
        )?;
        write_findings(
            f,
            Category::View,
            None,
            Findings::of(&self.missing_views, &self.extra_views, &self.altered_views),
            wording,
        )?;
        write_findings(
            f,
            Category::Trigger,
            None,
            Findings::of(
                &self.missing_triggers,
                &self.extra_triggers,
                &self.altered_triggers,
            ),
            wording,
        )?;
        write_findings(
            f,
            Category::Table,
            None,
            Findings::of(&self.missing_tables, &self.extra_tables, &[]),
            wording,
        )?;

        // A table that drifted is described member by member instead of with a
        // single `definition differs` line, so its detail is rendered here.
        for (table, diff) in &self.table_diffs {
            diff.write_details(f, table, wording)?;
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

    /// Wording of `stage commit`: the declarative stage versus the live database
    /// the commit is about to seal.
    pub const COMMIT: Self = Self {
        expected: "the stage",
        live: "the database",
    };
}

/// A category of schema object whose findings [`SchemaDiff::write_details`]
/// renders.
///
/// The noun a sentence uses and whether the sentence is qualified with the owning
/// table are the only differences between the categories, so one category value
/// drives every section instead of a hand-written block per category.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Category {
    /// An enum type declared by the database.
    Enum,
    /// A view declared by the database.
    View,
    /// A trigger attached to a table.
    Trigger,
    /// A table of the schema.
    Table,
    /// A column of a table.
    Column,
    /// A check constraint of a table.
    Check,
    /// An index of a table.
    Index,
}

impl Category {
    /// The noun a sentence about this category uses.
    fn label(self) -> &'static str {
        match self {
            Self::Enum => "Enum",
            Self::View => "View",
            Self::Trigger => "Trigger",
            Self::Table => "Table",
            Self::Column => "column",
            Self::Check => "check",
            Self::Index => "index",
        }
    }

    /// Whether the category is declared inside a table, which qualifies the
    /// sentence with the owning table.
    fn is_member(self) -> bool {
        matches!(self, Self::Column | Self::Check | Self::Index)
    }
}

/// The subject of a finding: a top-level object, or an item qualified by the
/// table that declares it.
struct Subject<'a> {
    owner: Option<&'a str>,
    category: Category,
    name: &'a str,
}

impl fmt::Display for Subject<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let category = self.category.label();
        let name = self.name;
        match self.owner {
            Some(table) => write!(f, "Table `{table}`: {category} `{name}`"),
            None => write!(f, "{category} `{name}`"),
        }
    }
}

/// One category's findings, borrowed from either a [`SchemaDiff`] or a
/// [`TableDiff`].
#[derive(Debug, Clone, Copy)]
struct Findings<'a> {
    missing: &'a [String],
    extra: &'a [String],
    altered: &'a [String],
}

impl<'a> Findings<'a> {
    /// Borrows the three state lists of one category.
    fn of(missing: &'a [String], extra: &'a [String], altered: &'a [String]) -> Self {
        Self {
            missing,
            extra,
            altered,
        }
    }
}

/// Renders the missing, extra, and altered findings of one category.
///
/// Only the `missing` sentence distinguishes a top-level object (declared in the
/// expected catalog) from an item nested in a table (present in it), so every
/// category shares the remaining wording.
fn write_findings(
    f: &mut fmt::Formatter<'_>,
    category: Category,
    owner: Option<&str>,
    findings: Findings<'_>,
    wording: DiffWording,
) -> fmt::Result {
    let DiffWording { expected, live } = wording;
    let missing_verb = if category.is_member() {
        "is in"
    } else {
        "is declared in"
    };

    for name in findings.missing {
        let subject = Subject {
            owner,
            category,
            name: name.as_str(),
        };
        writeln!(
            f,
            "  - {subject} {missing_verb} {expected} but missing from {live}."
        )?;
    }
    for name in findings.extra {
        let subject = Subject {
            owner,
            category,
            name: name.as_str(),
        };
        writeln!(
            f,
            "  - {subject} exists in {live} but is missing from {expected}."
        )?;
    }
    for name in findings.altered {
        let subject = Subject {
            owner,
            category,
            name: name.as_str(),
        };
        writeln!(
            f,
            "  - {subject} definition differs between {live} and {expected}."
        )?;
    }

    Ok(())
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

    /// Renders the findings of one table, every sentence qualified by its name.
    fn write_details(
        &self,
        f: &mut fmt::Formatter<'_>,
        table: &str,
        wording: DiffWording,
    ) -> fmt::Result {
        write_findings(
            f,
            Category::Column,
            Some(table),
            Findings::of(
                &self.missing_columns,
                &self.extra_columns,
                &self.altered_columns,
            ),
            wording,
        )?;

        let DiffWording { expected, live } = wording;

        if self.primary_key_mismatch {
            writeln!(
                f,
                "  - Table `{table}`: primary key constraint differs between {live} and {expected}."
            )?;
        }
        if self.foreign_key_mismatch {
            writeln!(
                f,
                "  - Table `{table}`: foreign key constraints differ between {live} and {expected}."
            )?;
        }

        write_findings(
            f,
            Category::Check,
            Some(table),
            Findings::of(
                &self.missing_checks,
                &self.extra_checks,
                &self.altered_checks,
            ),
            wording,
        )?;
        write_findings(
            f,
            Category::Index,
            Some(table),
            Findings::of(
                &self.missing_indexes,
                &self.extra_indexes,
                &self.altered_indexes,
            ),
            wording,
        )?;

        Ok(())
    }
}

/// The three states a named entry can be in, together with the entries both
/// catalogs declare.
struct NamedEntries<'a, V> {
    /// Names only the expected catalog declares.
    missing: Vec<String>,
    /// Names only the live catalog declares.
    extra: Vec<String>,
    /// Entries both catalogs declare, in name order.
    shared: Vec<(&'a str, &'a V, &'a V)>,
}

impl<'a, V> NamedEntries<'a, V> {
    /// Splits the comparison into the three state lists, using `altered` to decide
    /// which shared entries changed their definition.
    fn into_states(
        self,
        altered: impl Fn(&V, &V) -> bool,
    ) -> (Vec<String>, Vec<String>, Vec<String>) {
        let altered = self
            .shared
            .iter()
            .filter(|(_, live, expected)| altered(live, expected))
            .map(|(name, _, _)| (*name).to_owned())
            .collect();
        (self.missing, self.extra, altered)
    }
}

/// Groups two catalogs' entries by name and reports which names only one side
/// declares, together with the entries both sides share.
///
/// Every category of the comparison goes through this single traversal, so the
/// unreachable `(None, None)` state is matched once instead of once per category.
fn compare_named_entries<'a, V>(
    live: &'a [V],
    expected: &'a [V],
    name: impl Fn(&'a V) -> &'a str,
) -> NamedEntries<'a, V> {
    let live_by_name: BTreeMap<&'a str, &'a V> =
        live.iter().map(|entry| (name(entry), entry)).collect();
    let expected_by_name: BTreeMap<&'a str, &'a V> = expected
        .iter()
        .map(|entry| (name(entry), entry))
        .collect();

    let names: BTreeSet<&'a str> = live_by_name
        .keys()
        .copied()
        .chain(expected_by_name.keys().copied())
        .collect();

    let mut comparison = NamedEntries {
        missing: Vec::new(),
        extra: Vec::new(),
        shared: Vec::new(),
    };

    for entry_name in names {
        match (
            live_by_name.get(entry_name).copied(),
            expected_by_name.get(entry_name).copied(),
        ) {
            (Some(live_entry), Some(expected_entry)) => {
                comparison
                    .shared
                    .push((entry_name, live_entry, expected_entry));
            }
            (Some(_), None) => comparison.extra.push(entry_name.to_owned()),
            (None, Some(_)) => comparison.missing.push(entry_name.to_owned()),
            (None, None) => {}
        }
    }

    comparison
}

/// Computes the structural differences between the `live` and `expected` catalogs.
pub(crate) fn compute_catalog_diff(live: &SchemaCatalog, expected: &SchemaCatalog) -> SchemaDiff {
    let mut diff = SchemaDiff::default();

    let (missing, extra, altered) = compare_named_entries(
        &live.enums,
        &expected.enums,
        |entry: &EnumDef| entry.name.as_str(),
    )
    .into_states(|live, expected| live.variants != expected.variants);
    diff.missing_enums = missing;
    diff.extra_enums = extra;
    diff.altered_enums = altered;

    let (missing, extra, altered) = compare_named_entries(
        &live.views,
        &expected.views,
        |entry: &ViewDef| entry.name.as_str(),
    )
    .into_states(|live, expected| live.definition != expected.definition);
    diff.missing_views = missing;
    diff.extra_views = extra;
    diff.altered_views = altered;

    let (missing, extra, altered) = compare_named_entries(
        &live.triggers,
        &expected.triggers,
        |entry: &TriggerDef| entry.name.as_str(),
    )
    .into_states(|live, expected| {
        live.table_name != expected.table_name || live.definition != expected.definition
    });
    diff.missing_triggers = missing;
    diff.extra_triggers = extra;
    diff.altered_triggers = altered;

    // A table that drifted is described by the members it changed, so the shared
    // entries stay around for [`compute_table_diff`] instead of collapsing into an
    // `altered` name list.
    let tables = compare_named_entries(&live.tables, &expected.tables, |entry: &TableDef| {
        entry.name.as_str()
    });
    diff.missing_tables = tables.missing;
    diff.extra_tables = tables.extra;
    for (table_name, live_table, expected_table) in tables.shared {
        let table_diff = compute_table_diff(live_table, expected_table);
        if !table_diff.is_empty() {
            diff.table_diffs.insert(table_name.to_owned(), table_diff);
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

    let (missing, extra, altered) = compare_named_entries(
        &live.columns,
        &expected.columns,
        |entry: &ColumnDef| entry.name.as_str(),
    )
    .into_states(|live, expected| {
        live.type_kind != expected.type_kind
            || live.nullable != expected.nullable
            || live.default != expected.default
    });
    diff.missing_columns = missing;
    diff.extra_columns = extra;
    diff.altered_columns = altered;

    if live.primary_key != expected.primary_key {
        diff.primary_key_mismatch = true;
    }

    let (missing, extra, altered) = compare_named_entries(
        &live.checks,
        &expected.checks,
        |entry: &CheckDef| entry.name.as_str(),
    )
    .into_states(|live, expected| {
        normalize_expression(&live.expression) != normalize_expression(&expected.expression)
    });
    diff.missing_checks = missing;
    diff.extra_checks = extra;
    diff.altered_checks = altered;

    let live_foreign_keys: BTreeMap<&str, &ForeignKeyDef> = live
        .foreign_keys
        .iter()
        .map(|fk| (fk.name.as_str(), fk))
        .collect();
    let expected_foreign_keys: BTreeMap<&str, &ForeignKeyDef> = expected
        .foreign_keys
        .iter()
        .map(|fk| (fk.name.as_str(), fk))
        .collect();
    if live_foreign_keys != expected_foreign_keys {
        diff.foreign_key_mismatch = true;
    }

    let (missing, extra, altered) = compare_named_entries(
        &live.indexes,
        &expected.indexes,
        |entry: &IndexDef| entry.name.as_str(),
    )
    .into_states(|live, expected| {
        live.unique != expected.unique
            || live.columns != expected.columns
            || live.method != expected.method
            || live.predicate != expected.predicate
            || live.expression != expected.expression
    });
    diff.missing_indexes = missing;
    diff.extra_indexes = extra;
    diff.altered_indexes = altered;

    diff
}

#[cfg(test)]
mod tests;
