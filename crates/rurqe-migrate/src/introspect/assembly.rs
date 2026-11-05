//! Assembly of a normalized catalog out of the raw rows of any engine.
//!
//! Every engine reports the same objects with the same attributes, so the rows
//! are grouped by the composite `(schema, name)` identity of the object that
//! declares them and folded into one definition per object: a same-named table,
//! enum, view, or trigger of two schemas stays two definitions instead of
//! collapsing into one.

use std::collections::BTreeMap;

use crate::catalog::{
    CheckDef, ColumnDef, EnumDef, ForeignKeyDef, IndexDef, SchemaCatalog, TableDef, TriggerDef,
    TypeKind, ViewDef,
};

use super::metadata::{
    RawCatalogSections, RawColumnMetadata, RawEnumMetadata, RawForeignKeyMetadata,
    RawIndexMetadata, RawPrimaryKeyMetadata, RawTableMetadata, TableKey,
};
use super::type_mapping::map_sql_type_to_kind;

/// The enum types of the introspected selection, keyed by the composite
/// `(schema, name)` identity the catalog records for each of them.
///
/// Two schemas may declare the very same type name with different labels, so the
/// registry keeps those types apart exactly as `schema.lock` does.
type EnumRegistry = BTreeMap<(Option<String>, String), Vec<String>>;

/// The canonical catalog identity of the enum type `name` inside `schema`.
fn enum_key(schema: Option<&str>, name: &str) -> (Option<String>, String) {
    (schema.map(|schema| schema.to_owned()), name.to_owned())
}

/// Resolves the catalog type of one introspected column.
///
/// PostgreSQL reports an array column as `ARRAY` and names its element type
/// through `udt_name`, prefixed with an underscore, so the element type is
/// classified on its own and wrapped back into [`TypeKind::Array`]. A
/// `USER-DEFINED` column names its type through `udt_name` alone, and a column
/// whose type is declared by the database itself is an enum of that name.
///
/// An enum type is resolved through the composite identity the catalog records,
/// because two schemas may declare the same type name: the column reports the
/// schema its type lives in through `udt_schema`, already reduced to the catalog
/// convention, and a name matching no enum type of that schema keeps the declared
/// spelling instead of being reported as an enum.
fn column_type_kind(column: &RawColumnMetadata, enums: &EnumRegistry) -> TypeKind {
    let udt = column
        .udt_name
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty());
    let udt_schema = column.udt_schema.as_deref();

    if column.data_type.trim().eq_ignore_ascii_case("ARRAY") {
        if let Some(element) = udt.and_then(|name| name.strip_prefix('_')) {
            return TypeKind::Array(Box::new(named_type_kind(element, udt_schema, enums)));
        }
    }

    match udt {
        Some(name) if enums.contains_key(&enum_key(udt_schema, name)) => {
            TypeKind::Enum(name.to_owned())
        }
        Some(name) if column.data_type.trim().eq_ignore_ascii_case("USER-DEFINED") => {
            named_type_kind(name, udt_schema, enums)
        }
        _ => map_sql_type_to_kind(&column.data_type),
    }
}

/// Resolves the type named by a `udt_name`, mapping an enum to its own kind.
///
/// A name that matches no enum type of its own schema keeps the declared spelling
/// through [`map_sql_type_to_kind`], which reports a type outside the built-in set
/// as [`TypeKind::Other`] instead of guessing.
fn named_type_kind(name: &str, schema: Option<&str>, enums: &EnumRegistry) -> TypeKind {
    if enums.contains_key(&enum_key(schema, name)) {
        TypeKind::Enum(name.to_owned())
    } else {
        map_sql_type_to_kind(name)
    }
}

/// Splits a SQL term list at the commas that separate its top-level terms.
///
/// Parentheses, single-quoted text, and double-quoted identifiers all nest a
/// comma inside a larger term, so only a comma written outside them ends one.
/// PostgreSQL hands the expressions of an index over as the rendering of
/// `pg_index.indexprs` and SQLite as the source text of its parenthesised column
/// list, so the same scan reduces either one to the terms it is made of.
pub(crate) fn split_top_level_terms(text: &str) -> Vec<&str> {
    let mut terms = Vec::new();
    let mut start = 0usize;
    let mut depth = 0usize;
    let mut quote: Option<char> = None;
    let mut characters = text.char_indices().peekable();

    while let Some((index, character)) = characters.next() {
        match quote {
            Some(active) => {
                if character == active {
                    // A doubled quote is an escaped quote inside the literal, not
                    // the end of it.
                    if characters.peek().map(|(_, next)| *next) == Some(active) {
                        characters.next();
                    } else {
                        quote = None;
                    }
                }
            }
            None => match character {
                '\'' | '"' => quote = Some(character),
                '(' => depth = depth.saturating_add(1),
                ')' => depth = depth.saturating_sub(1),
                ',' if depth == 0 => {
                    terms.push(text[start..index].trim());
                    start = index + character.len_utf8();
                }
                _ => {}
            },
        }
    }

    terms.push(text[start..].trim());
    terms
}

/// The functional expressions an engine rendered for an index.
///
/// PostgreSQL renders the expression list of an index with a `NULL` term standing
/// in for a key attribute that is a plain column, so those placeholders are
/// dropped and only the expressions the index really computes are kept. The
/// comparison ignores case, because the rendering spells the keyword in upper
/// case.
fn functional_expressions(text: &str) -> Option<String> {
    let expressions: Vec<&str> = split_top_level_terms(text)
        .into_iter()
        .filter(|term| !term.is_empty())
        .filter(|term| !term.eq_ignore_ascii_case("null"))
        .collect();

    if expressions.is_empty() {
        None
    } else {
        Some(expressions.join(", "))
    }
}

/// The composite identity of the table `table_name` inside `schema`.
fn table_key(schema: Option<&str>, table_name: &str) -> TableKey {
    (
        schema.map(|schema| schema.to_owned()),
        table_name.to_owned(),
    )
}

/// Folds flat index rows into one [`IndexDef`] per index.
///
/// Both `pg_index` and the SQLite `PRAGMA index_list` report one row per indexed
/// column, so the rows are collected in arrival order and a composite index keeps
/// the column order it was declared in. The attributes of an index are read from
/// the first row of it, because every row of one index reports the same value.
///
/// A row without a column name belongs to an expression index; it contributes no
/// column while the expression it carries still reaches the definition. The
/// expression text is reduced to the functional expressions the index computes, so
/// a placeholder standing in for a plain key column never reaches the definition.
/// The returned map is keyed by the composite identity of the owning table, so two
/// same-named tables of different schemas never share the indexes declared on
/// them.
pub fn fold_indexes(indexes: &[RawIndexMetadata]) -> BTreeMap<TableKey, Vec<IndexDef>> {
    let mut by_table: BTreeMap<TableKey, BTreeMap<String, IndexDef>> = BTreeMap::new();

    for row in indexes {
        let index = by_table
            .entry(table_key(row.schema.as_deref(), &row.table_name))
            .or_default()
            .entry(row.index_name.clone())
            .or_insert_with(|| {
                let mut index = IndexDef::new(&row.index_name, Vec::new(), row.unique);
                if let Some(method) = &row.method {
                    index = index.with_method(method);
                }
                if let Some(predicate) = &row.predicate {
                    index = index.with_predicate(predicate);
                }
                if let Some(expression) = row.expression.as_deref().and_then(functional_expressions)
                {
                    index = index.with_expression(expression);
                }
                index
            });

        if let Some(column) = &row.column_name {
            if !index.columns.contains(column) {
                index.columns.push(column.clone());
            }
        }
    }

    by_table
        .into_iter()
        .map(|(table, indexes)| (table, indexes.into_values().collect()))
        .collect()
}

/// Assembles a normalized [`SchemaCatalog`] from introspected metadata collections.
///
/// Foreign key rows sharing a constraint name on the same table are folded into a
/// single [`ForeignKeyDef`], preserving the order of the columns inside the key.
/// Index rows are folded the same way into one [`IndexDef`] per index.
///
/// Every member of a table is grouped by the composite `(schema, name)` identity of
/// the table that declares it, so same-named tables of different schemas never
/// share columns, keys, checks, or indexes.
pub fn catalog_from_metadata(
    tables: &[RawTableMetadata],
    columns: &[RawColumnMetadata],
    pks: &[RawPrimaryKeyMetadata],
    fks: &[RawForeignKeyMetadata],
    indexes: &[RawIndexMetadata],
    enums: &[RawEnumMetadata],
) -> SchemaCatalog {
    catalog_from_metadata_with_sections(
        tables,
        columns,
        pks,
        fks,
        indexes,
        enums,
        &RawCatalogSections::default(),
    )
}

/// Assembles a normalized [`SchemaCatalog`] including checks, views, and triggers.
///
/// The sections are attached to the catalog as they are reported, so an engine that
/// cannot observe one of them simply passes an empty slice.
///
/// Every entity keeps the schema it was reported in. The members of a table — its
/// columns, primary key, foreign keys, checks, and indexes — are grouped by the
/// composite identity of the table that declares them, enum types by the composite
/// identity of the type itself, and views and triggers by their own, so two
/// same-named objects of different schemas stay independent definitions instead of
/// collapsing into one. An entity reported without a schema belongs to the primary
/// schema and carries no attribution.
///
/// A foreign key keeps the schema of the table it references when the reference
/// crosses into another schema, and a column resolves its enum type through the
/// schema the type is declared in.
pub fn catalog_from_metadata_with_sections(
    tables: &[RawTableMetadata],
    columns: &[RawColumnMetadata],
    pks: &[RawPrimaryKeyMetadata],
    fks: &[RawForeignKeyMetadata],
    indexes: &[RawIndexMetadata],
    enums: &[RawEnumMetadata],
    sections: &RawCatalogSections<'_>,
) -> SchemaCatalog {
    let mut enums_by_name: EnumRegistry = BTreeMap::new();
    for e in enums {
        enums_by_name
            .entry((e.schema.clone(), e.name.clone()))
            .or_default()
            .push(e.variant.clone());
    }

    let mut columns_by_table: BTreeMap<TableKey, Vec<ColumnDef>> = BTreeMap::new();
    let mut pks_by_table: BTreeMap<TableKey, Vec<String>> = BTreeMap::new();
    let mut fks_by_table: BTreeMap<TableKey, BTreeMap<String, ForeignKeyDef>> = BTreeMap::new();
    let mut checks_by_table: BTreeMap<TableKey, Vec<CheckDef>> = BTreeMap::new();
    let mut indexes_by_table = fold_indexes(indexes);

    for col in columns {
        let kind = column_type_kind(col, &enums_by_name);
        let mut def = ColumnDef::new(&col.column_name, kind, col.is_nullable);
        if let Some(ref default) = col.column_default {
            def = def.with_default(default);
        }
        columns_by_table
            .entry(table_key(col.schema.as_deref(), &col.table_name))
            .or_default()
            .push(def);
    }

    for pk in pks {
        pks_by_table
            .entry(table_key(pk.schema.as_deref(), &pk.table_name))
            .or_default()
            .push(pk.column_name.clone());
    }

    for fk in fks {
        let constraint = fks_by_table
            .entry(table_key(fk.schema.as_deref(), &fk.table_name))
            .or_default()
            .entry(fk.name.clone())
            .or_insert_with(|| {
                let mut constraint =
                    ForeignKeyDef::new(&fk.name, Vec::new(), &fk.referenced_table, Vec::new())
                        .with_actions(fk.on_delete, fk.on_update);
                if let Some(schema) = &fk.referenced_schema {
                    constraint = constraint.with_referenced_schema(schema.as_str());
                }
                constraint
            });
        constraint.columns.push(fk.column_name.clone());
        constraint
            .referenced_columns
            .push(fk.referenced_column.clone());
    }

    for check in sections.checks {
        checks_by_table
            .entry(table_key(check.schema.as_deref(), &check.table_name))
            .or_default()
            .push(CheckDef::new(&check.name, &check.expression));
    }

    let mut result_tables = Vec::new();
    for table in tables {
        let identity = table_key(table.schema.as_deref(), &table.name);
        let table_cols = columns_by_table.remove(&identity).unwrap_or_default();
        let table_pk = pks_by_table.remove(&identity).unwrap_or_default();
        let table_fks: Vec<ForeignKeyDef> = fks_by_table
            .remove(&identity)
            .map(|constraints| constraints.into_values().collect())
            .unwrap_or_default();
        let table_checks = checks_by_table.remove(&identity).unwrap_or_default();
        let table_indexes = indexes_by_table.remove(&identity).unwrap_or_default();

        let mut def = TableDef::new(&table.name, table_cols, table_pk);
        if let Some(schema) = &table.schema {
            def = def.with_schema(schema.as_str());
        }
        for check in table_checks {
            def = def.with_check(check);
        }
        for fk in table_fks {
            def = def.with_foreign_key(fk);
        }
        for index in table_indexes {
            def = def.with_index(index);
        }
        result_tables.push(def);
    }

    let mut result_enums = Vec::with_capacity(enums_by_name.len());
    for ((schema, name), variants) in enums_by_name {
        let mut definition = EnumDef::new(name, variants);
        if let Some(schema) = schema {
            definition = definition.with_schema(schema);
        }
        result_enums.push(definition);
    }

    let mut views = Vec::with_capacity(sections.views.len());
    for view in sections.views {
        let mut definition = ViewDef::new(&view.name, &view.definition);
        if let Some(schema) = &view.schema {
            definition = definition.with_schema(schema.as_str());
        }
        views.push(definition);
    }

    let mut triggers = Vec::with_capacity(sections.triggers.len());
    for trigger in sections.triggers {
        let mut definition =
            TriggerDef::new(&trigger.name, &trigger.table_name, &trigger.definition);
        if let Some(schema) = &trigger.schema {
            definition = definition.with_schema(schema.as_str());
        }
        triggers.push(definition);
    }

    SchemaCatalog::new(result_tables)
        .with_enums(result_enums)
        .with_views(views)
        .with_triggers(triggers)
}
