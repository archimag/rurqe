//! Canonical rendering of the `schema.lock` snapshot.
//!
//! The snapshot starts with the enum sections of the schema and continues with
//! its tables, so the enum type a column refers to is declared before the column
//! that references it.

use super::types::{push_quoted, ColumnDef, EnumDef, ForeignKeyDef, IndexDef, TableDef};

/// Version marker written into every `schema.lock` snapshot.
///
/// A v2 snapshot quotes and escapes the SQL text of a column default, so a default
/// carrying a quote, a backslash, a line break, or the words `null` and `default`
/// reads back as the default it is.
pub(super) const LOCK_HEADER: &str = "schema lock v2";

/// Version marker of the snapshots that wrote column defaults verbatim.
///
/// A v1 snapshot is still accepted, because it describes a schema exactly as well
/// as a v2 one; only the text of a default is written differently.
pub(super) const LEGACY_LOCK_HEADER: &str = "schema lock v1";

/// Section header introducing one enum declaration.
pub(super) const ENUM_HEADER: &str = "[[enum]]";

/// Renders the canonical `schema.lock` snapshot for `enums` and `tables`.
///
/// Enum sections and tables, and the columns, foreign keys, and indexes of every
/// table, are emitted sorted by name, so the snapshot depends only on the schema
/// content and never on the declaration order of the catalog. The labels of an
/// enum keep their declared order, because the order of the labels is part of the
/// enum type itself.
///
/// The SQL text of a column default is emitted as a quoted string whose quotes,
/// backslashes, and line breaks are escaped, so the text of a default never spans
/// a line of the snapshot and never reads as a nullability marker.
pub(super) fn render_lock(enums: &[EnumDef], tables: &[TableDef]) -> String {
    let mut out = String::new();
    out.push_str(LOCK_HEADER);
    out.push('\n');

    let mut ordered_enums: Vec<&EnumDef> = enums.iter().collect();
    ordered_enums.sort_by(|left, right| left.name.cmp(&right.name));

    for enum_def in ordered_enums {
        out.push('\n');
        out.push_str(ENUM_HEADER);
        out.push('\n');
        out.push_str("name = ");
        push_quoted(&mut out, &enum_def.name);
        out.push('\n');
        out.push_str("variants = ");
        push_variants(&mut out, &enum_def.variants);
        out.push('\n');
    }

    let mut ordered: Vec<&TableDef> = tables.iter().collect();
    ordered.sort_by(|left, right| left.name.cmp(&right.name));

    for table in ordered {
        out.push('\n');
        out.push_str("table ");
        push_quoted(&mut out, &table.name);
        out.push_str(" {\n");

        let mut columns: Vec<&ColumnDef> = table.columns.iter().collect();
        columns.sort_by(|left, right| left.name.cmp(&right.name));
        for column in columns {
            out.push_str("  column ");
            push_quoted(&mut out, &column.name);
            out.push_str(": ");
            out.push_str(&column.type_kind.canonical());
            out.push_str(if column.nullable { " null" } else { " not null" });
            if let Some(default) = &column.default {
                out.push_str(" default ");
                push_default(&mut out, default);
            }
            out.push('\n');
        }

        if !table.primary_key.is_empty() {
            out.push_str("  primary key ");
            push_quoted_list(&mut out, &table.primary_key);
            out.push('\n');
        }

        let mut foreign_keys: Vec<&ForeignKeyDef> = table.foreign_keys.iter().collect();
        foreign_keys.sort_by(|left, right| left.name.cmp(&right.name));
        for foreign_key in foreign_keys {
            out.push_str("  foreign key ");
            push_quoted(&mut out, &foreign_key.name);
            out.push(' ');
            push_quoted_list(&mut out, &foreign_key.columns);
            out.push_str(" references ");
            push_quoted(&mut out, &foreign_key.referenced_table);
            out.push(' ');
            push_quoted_list(&mut out, &foreign_key.referenced_columns);
            out.push('\n');
        }

        let mut indexes: Vec<&IndexDef> = table.indexes.iter().collect();
        indexes.sort_by(|left, right| left.name.cmp(&right.name));
        for index in indexes {
            out.push_str(if index.unique {
                "  unique index "
            } else {
                "  index "
            });
            push_quoted(&mut out, &index.name);
            out.push(' ');
            push_quoted_list(&mut out, &index.columns);
            out.push('\n');
        }

        out.push_str("}\n");
    }

    out
}

/// Appends a bracketed, comma-separated list of quoted enum labels.
fn push_variants(out: &mut String, variants: &[String]) {
    out.push('[');
    for (index, variant) in variants.iter().enumerate() {
        if index > 0 {
            out.push_str(", ");
        }
        push_quoted(out, variant);
    }
    out.push(']');
}

/// Appends a parenthesised, comma-separated list of quoted identifiers.
fn push_quoted_list(out: &mut String, idents: &[String]) {
    out.push('(');
    for (index, ident) in idents.iter().enumerate() {
        if index > 0 {
            out.push_str(", ");
        }
        push_quoted(out, ident);
    }
    out.push(')');
}

/// Appends `default` as a quoted string, escaping the characters that would
/// otherwise end the string or the line it sits on.
///
/// A quote doubles, a backslash doubles, and a line break becomes `\n` or `\r`, so
/// the SQL text of a default always stays on the line of its column and never
/// closes the quoted value early.
fn push_default(out: &mut String, default: &str) {
    out.push('"');
    for character in default.chars() {
        match character {
            '"' => out.push_str("\"\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            other => out.push(other),
        }
    }
    out.push('"');
}
