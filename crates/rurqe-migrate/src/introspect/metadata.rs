//! Raw catalog rows every engine reports, and the referential action they carry.
//!
//! A row spells one catalog object the way its engine reported it, and every row
//! carries the schema the object it describes belongs to. The catalog records a
//! schema attribute only for a secondary schema, so a row of the primary schema
//! reports `None` while a row of any other schema reports its name: the convention
//! is the one the catalog and `schema.lock` already follow, which keeps a
//! single-schema introspection byte-identical to the catalog recorded before
//! schemas were read at all.

use crate::catalog::ForeignKeyAction;

/// Raw table metadata row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawTableMetadata {
    /// The schema holding the table, `None` for the primary schema.
    pub schema: Option<String>,
    pub name: String,
}

/// Raw column metadata row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawColumnMetadata {
    /// The schema holding the column's table, `None` for the primary schema.
    pub schema: Option<String>,
    pub table_name: String,
    pub column_name: String,
    pub data_type: String,
    pub is_nullable: bool,
    pub column_default: Option<String>,
    pub udt_name: Option<String>,
    /// The schema of the column type, `None` for the primary schema.
    pub udt_schema: Option<String>,
}

/// Raw enum metadata row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawEnumMetadata {
    /// The schema holding the enum type, `None` for the primary schema.
    pub schema: Option<String>,
    pub name: String,
    pub variant: String,
}

/// Raw primary key constraint row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawPrimaryKeyMetadata {
    /// The schema holding the keyed table, `None` for the primary schema.
    pub schema: Option<String>,
    pub table_name: String,
    pub column_name: String,
}

/// Raw foreign key constraint row.
///
/// One row describes one key column, and the referential actions of the
/// constraint are repeated on every row of it, so folding the rows of a
/// constraint keeps the actions intact. The schema of the referenced table is
/// repeated the same way, and a row carries it only when the reference crosses
/// into another schema.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawForeignKeyMetadata {
    /// The schema holding the owning table, `None` for the primary schema.
    pub schema: Option<String>,
    pub name: String,
    pub table_name: String,
    pub column_name: String,
    pub referenced_table: String,
    pub referenced_column: String,
    pub referenced_schema: Option<String>,
    pub on_delete: ForeignKeyAction,
    pub on_update: ForeignKeyAction,
}

/// Raw secondary index row, one per indexed column.
///
/// The index attributes are repeated on every row of an index, and an index
/// declared over an expression reports no column name at all: `PRAGMA
/// index_info` and `pg_index` both leave the attribute unnamed for the positional
/// entry an expression occupies. The `expression` attribute therefore carries the
/// rendering of the index terms as the engine spells it, which the index folding
/// reduces to the functional expressions alone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawIndexMetadata {
    /// The schema holding the indexed table, `None` for the primary schema.
    pub schema: Option<String>,
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
    /// The schema holding the constrained table, `None` for the primary schema.
    pub schema: Option<String>,
    pub table_name: String,
    pub name: String,
    pub expression: String,
}

/// Raw view definition row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawViewMetadata {
    /// The schema holding the view, `None` for the primary schema.
    pub schema: Option<String>,
    pub name: String,
    pub definition: String,
}

/// Raw trigger definition row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawTriggerMetadata {
    /// The schema holding the trigger, `None` for the primary schema.
    pub schema: Option<String>,
    pub name: String,
    pub table_name: String,
    pub definition: String,
}

/// The catalog sections introspected alongside tables and their columns.
///
/// Bundling them keeps the assembler signatures readable while every engine still
/// reports the sections it can observe.
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
/// letters while SQLite spells the action out, so both spellings of every action
/// are understood and anything unfamiliar falls back to `NO ACTION`, which is the
/// SQL default of a foreign key.
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

/// The composite catalog identity of a table: the schema it belongs to, `None`
/// for the primary schema, and the name of the table inside that schema.
///
/// Two schemas may declare the very same table name, so every collection the
/// catalog assembles is grouped by this pair instead of the bare name.
pub type TableKey = (Option<String>, String);
