//! Canonical rendering of the `schema.lock` snapshot.

use super::types::{ColumnDef, ForeignKeyDef, IndexDef, TableDef};

/// Version marker written into every `schema.lock` snapshot.
pub(super) const LOCK_HEADER: &str = "schema lock v1";

/// Renders the canonical `schema.lock` snapshot for `tables`.
///
/// Tables, columns, foreign keys, and indexes are emitted sorted by name, so the
/// snapshot depends only on the schema content and never on the declaration
/// order of the catalog.
pub(super) fn render_lock(tables: &[TableDef]) -> String {
    let mut out = String::new();
    out.push_str(LOCK_HEADER);
    out.push('\n');

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
            out.push_str(column.type_kind.as_str());
            out.push_str(if column.nullable { " null" } else { " not null" });
            if let Some(default) = &column.default {
                out.push_str(" default ");
                out.push_str(default);
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

/// Appends `ident` to `out` as a double-quoted SQL identifier, doubling any
/// embedded quote so the rendering stays unambiguous.
fn push_quoted(out: &mut String, ident: &str) {
    out.push('"');
    for character in ident.chars() {
        if character == '"' {
            out.push('"');
        }
        out.push(character);
    }
    out.push('"');
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
