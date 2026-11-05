//! Assembly of a normalized [`SchemaCatalog`] from the SQLite introspection rows.

use std::collections::BTreeMap;

use rurqe_migrate::catalog::{
    CheckDef, ColumnDef, EnumDef, ForeignKeyDef, SchemaCatalog, TableDef, TriggerDef, TypeKind,
    ViewDef,
};

use rurqe_migrate::introspect::{
    fold_indexes, map_sqlite_declared_type_to_kind, RawCatalogSections, RawIndexMetadata, TableKey,
};

use super::check_enums::parse_sqlite_check_enums;
use super::checks::SqliteEnumColumn;
use super::{RawSqliteColumnMetadata, RawSqliteForeignKeyMetadata, RawSqliteTableMetadata};

/// Assembles a normalized [`SchemaCatalog`] from SQLite introspection results.
///
/// Column types follow the SQLite affinity rules of
/// [`map_sqlite_declared_type_to_kind`], so every integer column is reported as
/// an [`Int64`](rurqe_migrate::catalog::TypeKind::Int64) no matter how its
/// declared type is spelled.
///
/// Columns whose `pk_position` is greater than zero form the primary key, ordered
/// by their position in the key. `PRAGMA foreign_key_list` rows sharing a
/// constraint id are folded into one [`ForeignKeyDef`] named after the owning
/// table and that id, so composite foreign keys stay a single constraint. The
/// referential actions of the key are carried into the definition.
///
/// Index rows are folded into one [`IndexDef`](rurqe_migrate::catalog::IndexDef)
/// per index, keeping the column order of the index declaration.
///
/// `PRAGMA foreign_key_list` reports a `NULL` target column for a key that
/// references the implicit primary key of its parent table; such a target is
/// resolved against the parent key columns by position instead of being kept as
/// an empty name.
pub fn catalog_from_sqlite_metadata(
    tables: &[RawSqliteTableMetadata],
    columns: &[RawSqliteColumnMetadata],
    foreign_keys: &[RawSqliteForeignKeyMetadata],
    indexes: &[RawIndexMetadata],
) -> SchemaCatalog {
    catalog_from_sqlite_metadata_with_sections(
        tables,
        columns,
        foreign_keys,
        indexes,
        &RawCatalogSections::default(),
    )
}

/// Assembles a normalized [`SchemaCatalog`] including checks, views, and triggers.
///
/// The enum columns of the schema are recovered here by scanning the DDL of every
/// table once. A caller that already recovered them while collecting the check
/// constraints of the same statements hands them over through
/// [`catalog_from_sqlite_metadata_with_enums`] instead, so the assembly never
/// reads a statement a collection pass already read.
pub fn catalog_from_sqlite_metadata_with_sections(
    tables: &[RawSqliteTableMetadata],
    columns: &[RawSqliteColumnMetadata],
    foreign_keys: &[RawSqliteForeignKeyMetadata],
    indexes: &[RawIndexMetadata],
    sections: &RawCatalogSections<'_>,
) -> SchemaCatalog {
    let enum_columns = recover_enum_columns(tables, columns);

    catalog_from_sqlite_metadata_with_enums(
        tables,
        columns,
        foreign_keys,
        indexes,
        sections,
        &enum_columns,
    )
}

/// Assembles a normalized [`SchemaCatalog`] from already recovered enum columns.
///
/// The catalog of a live SQLite database is built from the enum columns its check
/// collector recovered while scanning each DDL, so the assembly never lexes a
/// statement the collection pass has already parsed.
pub(super) fn catalog_from_sqlite_metadata_with_enums(
    tables: &[RawSqliteTableMetadata],
    columns: &[RawSqliteColumnMetadata],
    foreign_keys: &[RawSqliteForeignKeyMetadata],
    indexes: &[RawIndexMetadata],
    sections: &RawCatalogSections<'_>,
    enum_columns: &[SqliteEnumColumn],
) -> SchemaCatalog {
    // A SQLite file has no schema layer, so every entity of its catalog belongs
    // to the primary schema and carries no attribution. The grouping keys keep
    // the composite `(schema, name)` identity the catalog records, with the
    // schema always absent, so same-named objects of one file stay apart exactly
    // as they do in an engine that has schemas.
    let mut enums_by_name: BTreeMap<(Option<String>, String), Vec<String>> = BTreeMap::new();
    let mut check_enums: BTreeMap<(String, String), String> = BTreeMap::new();
    for recovered in enum_columns {
        let candidate = recovered.column_name.clone();
        let variants = recovered.variants.clone();
        let key = (None, candidate.clone());
        let enum_name = match enums_by_name.get(&key) {
            Some(existing_variants) if existing_variants == &variants => candidate,
            None => candidate,
            Some(_) => format!("{}_{}_enum", recovered.table_name, recovered.column_name),
        };
        // An enum whose value set differs from the one already declared under the
        // same column name keeps its own identity through the disambiguated name.
        enums_by_name
            .entry((None, enum_name.clone()))
            .or_insert_with(|| variants);
        check_enums.insert(
            (recovered.table_name.clone(), recovered.column_name.clone()),
            enum_name,
        );
    }

    let mut columns_by_table: BTreeMap<TableKey, Vec<ColumnDef>> = BTreeMap::new();
    let mut fks_by_table: BTreeMap<TableKey, BTreeMap<i64, ForeignKeyDef>> = BTreeMap::new();
    let mut checks_by_table: BTreeMap<TableKey, Vec<CheckDef>> = BTreeMap::new();
    let mut indexes_by_table = fold_indexes(indexes);

    // Key columns in key order: they name both the assembled primary keys and
    // the target of a foreign key that references an implicit parent key.
    let mut primary_keys: BTreeMap<&str, Vec<(i64, &str)>> = BTreeMap::new();

    for column in columns {
        let kind = if let Some(enum_name) =
            check_enums.get(&(column.table_name.clone(), column.column_name.clone()))
        {
            TypeKind::Enum(enum_name.clone())
        } else {
            map_sqlite_declared_type_to_kind(&column.data_type)
        };
        let mut def = ColumnDef::new(&column.column_name, kind, !column.not_null);
        if let Some(ref default) = column.default_value {
            def = def.with_default(default);
        }
        columns_by_table
            .entry(table_key(&column.table_name))
            .or_default()
            .push(def);

        if column.pk_position > 0 {
            primary_keys
                .entry(column.table_name.as_str())
                .or_default()
                .push((column.pk_position, column.column_name.as_str()));
        }
    }

    for key in primary_keys.values_mut() {
        key.sort_by_key(|(position, _)| *position);
    }

    for fk in foreign_keys {
        let constraint = fks_by_table
            .entry(table_key(&fk.table_name))
            .or_default()
            .entry(fk.id)
            .or_insert_with(|| {
                ForeignKeyDef::new(
                    format!("{}_{}_fkey", fk.table_name, fk.id),
                    Vec::new(),
                    &fk.referenced_table,
                    Vec::new(),
                )
                .with_actions(fk.on_delete, fk.on_update)
            });

        // `PRAGMA foreign_key_list` reports a NULL target column for a key that
        // references the implicit primary key of the parent table, so the name
        // is recovered from the introspected key columns by position.
        let referenced_column = if fk.to_column.is_empty() {
            implicit_referenced_column(
                &primary_keys,
                &fk.referenced_table,
                constraint.columns.len(),
            )
            .unwrap_or_default()
        } else {
            fk.to_column.clone()
        };
        constraint.columns.push(fk.from_column.clone());
        constraint.referenced_columns.push(referenced_column);
    }

    for check in sections.checks {
        checks_by_table
            .entry(table_key(&check.table_name))
            .or_default()
            .push(CheckDef::new(&check.name, &check.expression));
    }

    let mut result_tables = Vec::new();
    for table in tables {
        let identity = table_key(&table.name);
        let table_columns = columns_by_table.remove(&identity).unwrap_or_default();
        let primary_key: Vec<String> = primary_keys
            .get(table.name.as_str())
            .map(|key| key.iter().map(|(_, name)| (*name).to_owned()).collect())
            .unwrap_or_default();
        let table_foreign_keys: Vec<ForeignKeyDef> = fks_by_table
            .remove(&identity)
            .map(|constraints| constraints.into_values().collect())
            .unwrap_or_default();
        let table_checks = checks_by_table.remove(&identity).unwrap_or_default();
        let table_indexes = indexes_by_table.remove(&identity).unwrap_or_default();

        let mut def = TableDef::new(&table.name, table_columns, primary_key);
        for check in table_checks {
            def = def.with_check(check);
        }
        for fk in table_foreign_keys {
            def = def.with_foreign_key(fk);
        }
        for index in table_indexes {
            def = def.with_index(index);
        }
        result_tables.push(def);
    }

    let mut enums: Vec<EnumDef> = enums_by_name
        .into_iter()
        .map(|((_schema, name), variants)| EnumDef::new(name, variants))
        .collect();
    enums.sort_by(|a, b| a.name.cmp(&b.name));

    let views: Vec<ViewDef> = sections
        .views
        .iter()
        .map(|view| ViewDef::new(&view.name, &view.definition))
        .collect();
    let triggers: Vec<TriggerDef> = sections
        .triggers
        .iter()
        .map(|trigger| TriggerDef::new(&trigger.name, &trigger.table_name, &trigger.definition))
        .collect();

    SchemaCatalog::new(result_tables)
        .with_enums(enums)
        .with_views(views)
        .with_triggers(triggers)
}

/// Recovers the enum columns declared by the tables of the schema.
///
/// Each DDL is scanned once, in the order the tables are reported, so the enum
/// names collide and disambiguate exactly as the catalog assembler expects.
fn recover_enum_columns(
    tables: &[RawSqliteTableMetadata],
    columns: &[RawSqliteColumnMetadata],
) -> Vec<SqliteEnumColumn> {
    let mut enum_columns = Vec::new();

    for table in tables {
        let Some(sql) = table.sql.as_deref() else {
            continue;
        };
        let known: Vec<&str> = columns
            .iter()
            .filter(|column| column.table_name == table.name)
            .map(|column| column.column_name.as_str())
            .collect();

        for (column_name, variants) in parse_sqlite_check_enums(sql, &known) {
            enum_columns.push(SqliteEnumColumn {
                table_name: table.name.clone(),
                column_name,
                variants,
            });
        }
    }

    enum_columns
}

/// Names the column a foreign key references when SQLite omits it.
///
/// A key that targets the implicit primary key of its parent table is reported
/// without a target column, and `ordinal` selects the matching key column.
fn implicit_referenced_column(
    primary_keys: &BTreeMap<&str, Vec<(i64, &str)>>,
    referenced_table: &str,
    ordinal: usize,
) -> Option<String> {
    primary_keys
        .get(referenced_table)
        .and_then(|key| key.get(ordinal))
        .map(|(_, name)| (*name).to_owned())
}

/// The composite catalog identity of a SQLite table.
///
/// A SQLite file has no schema layer, so the schema part of the identity is
/// always absent and the name alone keeps the tables of the file apart.
fn table_key(table_name: &str) -> TableKey {
    (None, table_name.to_owned())
}
