//! Rendering of a validated table plan into Rust source text.

use rurqe_migrate::catalog::SchemaCatalog;

use super::naming::escape_literal;
use super::plan::{build_plan, TablePlan};
use super::{CodegenError, CodegenOptions};

/// Generates Rust schema source code from an engine-neutral [`SchemaCatalog`].
///
/// Names are sanitized into valid Rust identifiers (`2fa` becomes `_2fa`,
/// `user.email` becomes `user_email`) and every generated item is registered in
/// its namespace, so two schema names that reduce to the same identifier are
/// reported as [`CodegenError::IdentifierCollision`] instead of emitting a
/// module with duplicate definitions.
pub fn generate_schema_rs(
    catalog: &SchemaCatalog,
    options: &CodegenOptions,
) -> Result<String, CodegenError> {
    let tables = build_plan(catalog, options)?;

    let mut out = String::new();

    if let Some(ref doc) = options.doc_comment {
        for line in doc.lines() {
            if line.is_empty() {
                out.push_str("//!\n");
            } else {
                out.push_str("//! ");
                out.push_str(line);
                out.push('\n');
            }
        }
        out.push('\n');
    }

    out.push_str("#![allow(dead_code)]\n\n");
    out.push_str("use rurqe::prelude::*;\n\n");

    // Embed the deterministic SHA-256 schema hash
    out.push_str(&catalog.hash_const());
    out.push_str("\n\n");

    for table in &tables {
        write_table(&mut out, table, options);
    }

    Ok(out)
}

/// Renders one `table!` block and its optional row record.
fn write_table(out: &mut String, table: &TablePlan<'_>, options: &CodegenOptions) {
    out.push_str("table! {\n");
    out.push_str("    /// The `");
    out.push_str(table.name);
    out.push_str("` table.\n");
    out.push_str("    ");
    out.push_str(&table.ident);
    out.push_str(" => \"");
    out.push_str(&escape_literal(table.name));
    out.push_str("\" {\n");

    for column in &table.columns {
        out.push_str("        ");
        out.push_str(&column.marker_ident);
        out.push_str(" => \"");
        out.push_str(&escape_literal(column.name));
        out.push_str("\": ");
        out.push_str(&column.rust_type);
        out.push_str(",\n");
    }

    out.push_str("    }\n");
    out.push_str("}\n\n");

    if !options.generate_records {
        return;
    }

    out.push_str("/// Strongly typed row record for `");
    out.push_str(table.name);
    out.push_str("`.\n");
    out.push_str("#[derive(Debug, Clone, PartialEq)]\n");
    out.push_str("pub struct ");
    out.push_str(&table.record_ident);
    out.push_str(" {\n");

    for column in &table.columns {
        out.push_str("    pub ");
        out.push_str(&column.field_ident);
        out.push_str(": ");
        out.push_str(&column.rust_type);
        out.push_str(",\n");
    }
    out.push_str("}\n\n");

    let arity = table.columns.len().to_string();

    out.push_str("impl FromRow for ");
    out.push_str(&table.record_ident);
    out.push_str(" {\n");
    out.push_str("    fn try_from_row(values: &[SqlVal]) -> Result<Self, RowDecodeError> {\n");
    out.push_str("        if values.len() != ");
    out.push_str(&arity);
    out.push_str(" {\n");
    out.push_str("            return Err(RowDecodeError::ArityMismatch {\n");
    out.push_str("                expected: ");
    out.push_str(&arity);
    out.push_str(",\n");
    out.push_str("                actual: values.len(),\n");
    out.push_str("            });\n");
    out.push_str("        }\n");
    out.push_str("        Ok(Self {\n");

    for (index, column) in table.columns.iter().enumerate() {
        out.push_str("            ");
        out.push_str(&column.field_ident);
        out.push_str(": FromSqlValue::from_sql_value(&values[");
        out.push_str(&index.to_string());
        out.push_str("])?,\n");
    }

    out.push_str("        })\n");
    out.push_str("    }\n");
    out.push_str("}\n\n");
}

#[cfg(test)]
mod tests {
    use super::*;
    use rurqe_migrate::catalog::{ColumnDef, TableDef, TypeKind};

    #[test]
    fn codegen_renders_deterministic_schema() {
        let catalog = SchemaCatalog::new(vec![TableDef::new(
            "users",
            vec![
                ColumnDef::new("id", TypeKind::Int64, false),
                ColumnDef::new("name", TypeKind::Text, false),
                ColumnDef::new("email", TypeKind::Text, true),
            ],
            vec!["id".to_owned()],
        )]);

        let code =
            generate_schema_rs(&catalog, &CodegenOptions::default()).expect("codegen succeeds");

        assert!(code.contains("pub const SCHEMA_HASH: &str = "));
        assert!(code.contains("Users => \"users\""));
        assert!(code.contains("UsersId => \"id\": i64"));
        assert!(code.contains("UsersEmail => \"email\": Option<String>"));
        assert!(code.contains("pub struct UsersRecord"));
        assert!(code.contains("impl FromRow for UsersRecord"));
        assert!(
            code.contains("fn try_from_row(values: &[SqlVal]) -> Result<Self, RowDecodeError>")
        );
        assert!(code.contains("RowDecodeError::ArityMismatch"));
    }

    #[test]
    fn generated_markers_survive_unusual_names() {
        let catalog = SchemaCatalog::new(vec![TableDef::new(
            "user.profiles",
            vec![ColumnDef::new("2fa enabled", TypeKind::Boolean, false)],
            Vec::new(),
        )]);

        let code =
            generate_schema_rs(&catalog, &CodegenOptions::default()).expect("codegen succeeds");

        assert!(code.contains("UserProfiles => \"user.profiles\""));
        assert!(code.contains("UserProfiles_2faEnabled => \"2fa enabled\": bool"));
        assert!(code.contains("pub _2fa_enabled: bool,"));
    }

    #[test]
    fn generated_records_carry_the_overridden_types() {
        let catalog = SchemaCatalog::new(vec![TableDef::new(
            "users",
            vec![
                ColumnDef::new("id", TypeKind::Uuid, false),
                ColumnDef::new("deleted_at", TypeKind::Timestamp, true),
            ],
            Vec::new(),
        )]);

        let mut options = CodegenOptions::default();
        options
            .type_overrides
            .insert("users.id".to_owned(), "uuid::Uuid".to_owned());
        options.type_overrides.insert(
            "users.deleted_at".to_owned(),
            "Option<chrono::NaiveDateTime>".to_owned(),
        );

        let code = generate_schema_rs(&catalog, &options).expect("codegen succeeds");

        assert!(code.contains("UsersId => \"id\": uuid::Uuid"));
        assert!(code.contains("pub id: uuid::Uuid,"));
        assert!(code.contains("UsersDeletedAt => \"deleted_at\": Option<chrono::NaiveDateTime>"));
        assert!(!code.contains("Option<Option<"));
    }

    #[test]
    fn records_are_omitted_on_request() {
        let catalog = SchemaCatalog::new(vec![TableDef::new(
            "users",
            vec![ColumnDef::new("id", TypeKind::Int64, false)],
            Vec::new(),
        )]);

        let options = CodegenOptions {
            generate_records: false,
            ..CodegenOptions::default()
        };
        let code = generate_schema_rs(&catalog, &options).expect("codegen succeeds");

        assert!(code.contains("Users => \"users\""));
        assert!(!code.contains("pub struct UsersRecord"));
    }
}
