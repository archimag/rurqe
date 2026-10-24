//! Engine-independent raw metadata rows, SQL type mapping, and catalog assembly.

use std::collections::BTreeMap;

use rurqe_migrate::catalog::{
    CheckDef, ColumnDef, EnumDef, ForeignKeyAction, ForeignKeyDef, IndexDef, SchemaCatalog,
    TableDef, TriggerDef, TypeKind, ViewDef,
};

/// Raw table metadata row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawTableMetadata {
    pub name: String,
}

/// Raw column metadata row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawColumnMetadata {
    pub table_name: String,
    pub column_name: String,
    pub data_type: String,
    pub is_nullable: bool,
    pub column_default: Option<String>,
    pub udt_name: Option<String>,
}

/// Raw enum metadata row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawEnumMetadata {
    pub name: String,
    pub variant: String,
}

/// Raw primary key constraint row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawPrimaryKeyMetadata {
    pub table_name: String,
    pub column_name: String,
}

/// Raw foreign key constraint row.
///
/// One row describes one key column, and the referential actions of the
/// constraint are repeated on every row of it, so folding the rows of a
/// constraint keeps the actions intact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawForeignKeyMetadata {
    pub name: String,
    pub table_name: String,
    pub column_name: String,
    pub referenced_table: String,
    pub referenced_column: String,
    pub on_delete: ForeignKeyAction,
    pub on_update: ForeignKeyAction,
}

/// Raw secondary index row, one per indexed column.
///
/// The index attributes are repeated on every row of an index, and an index
/// declared over an expression reports no column name at all: `PRAGMA
/// index_info` and `pg_index` both leave the attribute unnamed for the
/// positional entry an expression occupies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawIndexMetadata {
    pub table_name: String,
    pub index_name: String,
    pub column_name: Option<String>,
    pub unique: bool,
    pub method: Option<String>,
    pub predicate: Option<String>,
    pub expression: Option<String>,
}

/// Raw check constraint row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawCheckMetadata {
    pub table_name: String,
    pub name: String,
    pub expression: String,
}

/// Raw view definition row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawViewMetadata {
    pub name: String,
    pub definition: String,
}

/// Raw trigger definition row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawTriggerMetadata {
    pub name: String,
    pub table_name: String,
    pub definition: String,
}

/// The catalog sections introspected alongside tables and their columns.
///
/// Bundling them keeps the assembler signatures readable while every engine
/// still reports the sections it can observe.
#[derive(Debug, Default, Clone, Copy)]
pub struct RawCatalogSections<'a> {
    /// The check constraints of the schema.
    pub checks: &'a [RawCheckMetadata],
    /// The views of the schema.
    pub views: &'a [RawViewMetadata],
    /// The triggers of the schema.
    pub triggers: &'a [RawTriggerMetadata],
}

/// Reads a referential action reported by an engine.
///
/// PostgreSQL reports the action as one of its `confdeltype` / `confupdtype`
/// letters while SQLite spells the action out, so both spellings of every
/// action are understood and anything unfamiliar falls back to `NO ACTION`,
/// which is the SQL default of a foreign key.
pub fn parse_foreign_key_action(text: &str) -> ForeignKeyAction {
    let lower = text.trim().to_ascii_lowercase();

    match lower.as_str() {
        "r" | "restrict" => ForeignKeyAction::Restrict,
        "c" | "cascade" => ForeignKeyAction::Cascade,
        "n" | "set null" => ForeignKeyAction::SetNull,
        "d" | "set default" => ForeignKeyAction::SetDefault,
        _ => ForeignKeyAction::NoAction,
    }
}

/// Maps a raw SQL database data type string to an engine-neutral [`TypeKind`].
///
/// The mapping serves engines that report parameter-free type names, as
/// PostgreSQL does through `information_schema.columns.data_type`, and stays
/// tolerant of a parameterised spelling such as `varchar(255)`. A type outside
/// the built-in set keeps its declared spelling, type parameters included, so a
/// `numeric(10,2)` column never compares equal to a `numeric(18,4)` one inside
/// `schema.lock`.
///
/// An array spelling such as `text[]` maps to [`TypeKind::Array`] of the type
/// its elements are declared as.
pub fn map_sql_type_to_kind(sql_type: &str) -> TypeKind {
    let declared = sql_type.trim();

    // An array is spelled as its element type followed by `[]`, so the suffix is
    // peeled off before the element type itself is classified.
    if let Some(element) = declared.strip_suffix("[]") {
        return TypeKind::Array(Box::new(map_sql_type_to_kind(element)));
    }

    let lower = strip_type_parameters(declared).to_ascii_lowercase();

    match lower.as_str() {
        "boolean" | "bool" => TypeKind::Boolean,
        "smallint" | "int2" | "smallserial" => TypeKind::Int32,
        "integer" | "int" | "int4" | "serial" => TypeKind::Int32,
        "bigint" | "int8" | "bigserial" => TypeKind::Int64,
        "double precision" | "float8" | "real" | "float4" => TypeKind::Float64,
        "text" | "character varying" | "varchar" | "char" | "character" | "citext" => {
            TypeKind::Text
        }
        "bytea" | "blob" => TypeKind::Bytes,
        "uuid" => TypeKind::Uuid,
        "json" | "jsonb" => TypeKind::Json,
        "timestamp without time zone"
        | "timestamp with time zone"
        | "timestamp"
        | "timestamptz" => TypeKind::Timestamp,
        "date" => TypeKind::Date,
        _ => TypeKind::Other(declared.to_ascii_lowercase()),
    }
}

/// Maps the declared type of a SQLite column onto an engine-neutral [`TypeKind`].
///
/// SQLite keeps the declared type verbatim and classifies it through *type
/// affinity* rather than a fixed type system, so `VARCHAR(255)`,
/// `UNSIGNED BIG INT`, and `BOOLEAN` all have to be recognised from their
/// spelling. The affinity rules of the SQLite documentation are applied in
/// order:
///
/// 1. a declared type containing `INT` has an integer affinity, and SQLite
///    stores every integer column as a 64-bit value, so it maps to
///    [`TypeKind::Int64`];
/// 2. a type containing `CHAR`, `CLOB`, or `TEXT` maps to [`TypeKind::Text`];
/// 3. a type containing `BLOB`, or an absent declared type, maps to
///    [`TypeKind::Bytes`];
/// 4. a type containing `REAL`, `FLOA`, or `DOUB` maps to [`TypeKind::Float64`].
///
/// Declared types naming a domain rather than a storage class (`BOOLEAN`,
/// `UUID`, `JSON`, `DATE`, `DATETIME`) are recognised next, because SQLite
/// stores their values as the text or integer the matching Rust type decodes.
/// Everything else has the numeric affinity of SQLite, which keeps a value
/// either as an integer or as a real depending on its magnitude and precision,
/// so no single Rust type reads such a column reliably; the declared spelling is
/// preserved through [`TypeKind::Other`] instead of guessing.
pub fn map_sqlite_declared_type_to_kind(sql_type: &str) -> TypeKind {
    let declared = sql_type.trim();
    let lower = strip_type_parameters(declared).to_ascii_lowercase();

    if lower.contains("int") {
        TypeKind::Int64
    } else if lower.contains("char") || lower.contains("clob") || lower.contains("text") {
        TypeKind::Text
    } else if lower.contains("blob") || lower.is_empty() {
        TypeKind::Bytes
    } else if lower.contains("real") || lower.contains("floa") || lower.contains("doub") {
        TypeKind::Float64
    } else if lower.contains("bool") {
        // SQLite keeps booleans as the integers 0 and 1, which the row decoder
        // accepts for a `bool` column.
        TypeKind::Boolean
    } else if lower.contains("uuid") {
        TypeKind::Uuid
    } else if lower.contains("json") {
        TypeKind::Json
    } else if lower.contains("timestamp") || lower.contains("datetime") {
        TypeKind::Timestamp
    } else if lower == "date" {
        TypeKind::Date
    } else {
        TypeKind::Other(declared.to_ascii_lowercase())
    }
}

/// Cuts a SQL type at its first parameter list and trims the result.
///
/// PostgreSQL reports parameter-free type names through `information_schema`,
/// while SQLite keeps the declared type verbatim, so `VARCHAR(255)` and
/// `DECIMAL(10, 2)` are reduced to `VARCHAR` and `DECIMAL` before they are
/// classified.
fn strip_type_parameters(sql_type: &str) -> &str {
    match sql_type.find('(') {
        Some(index) => sql_type[..index].trim(),
        None => sql_type.trim(),
    }
}

/// Resolves the catalog type of one introspected column.
///
/// PostgreSQL reports an array column as `ARRAY` and names its element type
/// through `udt_name`, prefixed with an underscore, so the element type is
/// classified on its own and wrapped back into [`TypeKind::Array`]. A
/// `USER-DEFINED` column names its type through `udt_name` alone, and a column
/// whose type is declared by the database itself is an enum of that name.
fn column_type_kind(column: &RawColumnMetadata, enums: &BTreeMap<String, Vec<String>>) -> TypeKind {
    let udt = column
        .udt_name
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty());

    if column.data_type.trim().eq_ignore_ascii_case("ARRAY") {
        if let Some(element) = udt.and_then(|name| name.strip_prefix('_')) {
            return TypeKind::Array(Box::new(named_type_kind(element, enums)));
        }
    }

    match udt {
        Some(name) if enums.contains_key(name) => TypeKind::Enum(name.to_owned()),
        Some(name) if column.data_type.trim().eq_ignore_ascii_case("USER-DEFINED") => {
            named_type_kind(name, enums)
        }
        _ => map_sql_type_to_kind(&column.data_type),
    }
}

/// Resolves the type named by a `udt_name`, mapping an enum to its own kind.
fn named_type_kind(name: &str, enums: &BTreeMap<String, Vec<String>>) -> TypeKind {
    if enums.contains_key(name) {
        TypeKind::Enum(name.to_owned())
    } else {
        map_sql_type_to_kind(name)
    }
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
/// returned map is keyed by owning table.
pub(super) fn fold_indexes(indexes: &[RawIndexMetadata]) -> BTreeMap<String, Vec<IndexDef>> {
    let mut by_table: BTreeMap<String, BTreeMap<String, IndexDef>> = BTreeMap::new();

    for row in indexes {
        let index = by_table
            .entry(row.table_name.clone())
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
                if let Some(expression) = &row.expression {
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
/// The sections are attached to the catalog as they are reported, so an engine
/// that cannot observe one of them simply passes an empty slice.
pub fn catalog_from_metadata_with_sections(
    tables: &[RawTableMetadata],
    columns: &[RawColumnMetadata],
    pks: &[RawPrimaryKeyMetadata],
    fks: &[RawForeignKeyMetadata],
    indexes: &[RawIndexMetadata],
    enums: &[RawEnumMetadata],
    sections: &RawCatalogSections<'_>,
) -> SchemaCatalog {
    let mut enums_by_name: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for e in enums {
        enums_by_name
            .entry(e.name.clone())
            .or_default()
            .push(e.variant.clone());
    }

    let mut columns_by_table: BTreeMap<String, Vec<ColumnDef>> = BTreeMap::new();
    let mut pks_by_table: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut fks_by_table: BTreeMap<String, BTreeMap<String, ForeignKeyDef>> = BTreeMap::new();
    let mut checks_by_table: BTreeMap<String, Vec<CheckDef>> = BTreeMap::new();
    let mut indexes_by_table = fold_indexes(indexes);

    for col in columns {
        let kind = column_type_kind(col, &enums_by_name);
        let mut def = ColumnDef::new(&col.column_name, kind, col.is_nullable);
        if let Some(ref default) = col.column_default {
            def = def.with_default(default);
        }
        columns_by_table
            .entry(col.table_name.clone())
            .or_default()
            .push(def);
    }

    for pk in pks {
        pks_by_table
            .entry(pk.table_name.clone())
            .or_default()
            .push(pk.column_name.clone());
    }

    for fk in fks {
        let constraint = fks_by_table
            .entry(fk.table_name.clone())
            .or_default()
            .entry(fk.name.clone())
            .or_insert_with(|| {
                ForeignKeyDef::new(&fk.name, Vec::new(), &fk.referenced_table, Vec::new())
                    .with_actions(fk.on_delete, fk.on_update)
            });
        constraint.columns.push(fk.column_name.clone());
        constraint
            .referenced_columns
            .push(fk.referenced_column.clone());
    }

    for check in sections.checks {
        checks_by_table
            .entry(check.table_name.clone())
            .or_default()
            .push(CheckDef::new(&check.name, &check.expression));
    }

    let mut result_tables = Vec::new();
    for table in tables {
        let table_cols = columns_by_table.remove(&table.name).unwrap_or_default();
        let table_pk = pks_by_table.remove(&table.name).unwrap_or_default();
        let table_fks: Vec<ForeignKeyDef> = fks_by_table
            .remove(&table.name)
            .map(|constraints| constraints.into_values().collect())
            .unwrap_or_default();
        let table_checks = checks_by_table.remove(&table.name).unwrap_or_default();

        let table_indexes = indexes_by_table.remove(&table.name).unwrap_or_default();

        let mut def = TableDef::new(&table.name, table_cols, table_pk);
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

    let result_enums: Vec<EnumDef> = enums_by_name
        .into_iter()
        .map(|(name, variants)| EnumDef::new(name, variants))
        .collect();

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
        .with_enums(result_enums)
        .with_views(views)
        .with_triggers(triggers)
}
