//! Rendering of a validated section plan into Rust source text.

use rurqe_migrate::catalog::SchemaCatalog;

use super::naming::{doc_text, escape_literal};
use super::plan::{build_plan, EnumPlan, SecondarySchemaPlan, TablePlan};
use super::{CodegenError, CodegenOptions};

/// Generates Rust schema source code from an engine-neutral [`SchemaCatalog`].
///
/// The entities of the primary schema are rendered flat at the top level of the
/// generated module, so a query addresses them through the bare markers exactly
/// as a single-schema catalog produces them. The entities of every secondary
/// schema are grouped into a `schema!` block declaring the marker of that schema
/// next to the module holding its type namespace, so a table of a secondary
/// schema is addressed as `Auth.Users` while its markers stay in the `auth`
/// module.
///
/// Names are sanitized into valid Rust identifiers (`2fa` becomes `_2fa`,
/// `user.email` becomes `user_email`) and every generated item is registered in
/// the namespace of its section, so two schema names that reduce to the same
/// identifier are reported as [`CodegenError::IdentifierCollision`] instead of
/// emitting a module with duplicate definitions.
pub fn generate_schema_rs(
    catalog: &SchemaCatalog,
    options: &CodegenOptions,
) -> Result<String, CodegenError> {
    let plan = build_plan(catalog, options)?;

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

    for enum_plan in &plan.primary.enums {
        out.push_str(&render_enum(enum_plan));
    }

    for table in &plan.primary.tables {
        out.push_str(&render_table_block(table));
        out.push_str(&render_record(table, options));
    }

    for schema in &plan.secondary {
        push_block(&mut out, &render_schema(schema, options), "");
    }

    Ok(out)
}

/// Renders one secondary schema as a `schema!` block.
///
/// The block states the SQL name of the schema once and forwards every table to
/// `table!` with the `in` clause, so the relation renders its qualifier while
/// the type namespace of the schema stays inside the module the block declares.
fn render_schema(schema: &SecondarySchemaPlan<'_>, options: &CodegenOptions) -> String {
    let mut out = String::new();

    out.push_str("schema! {\n");
    out.push_str("    /// The `");
    out.push_str(&doc_text(schema.name));
    out.push_str("` schema.\n");
    out.push_str("    ");
    out.push_str(&schema.ident);
    out.push_str(" => \"");
    out.push_str(&escape_literal(schema.name));
    out.push_str("\" as ");
    out.push_str(&schema.module);
    out.push_str(" {\n");

    for table in &schema.section.tables {
        push_block(&mut out, &render_table_block(table), "        ");
    }

    let items = render_items(schema, options);
    if !items.is_empty() {
        out.push_str("        @items {\n");
        push_block(&mut out, &items, "            ");
        out.push_str("        }\n");
    }

    out.push_str("    }\n");
    out.push_str("}\n\n");

    out
}

/// Renders the items a secondary schema declares inside its own module.
///
/// The module owns its type namespace, so it imports the prelude its items are
/// written against instead of inheriting the import of the module around it.
fn render_items(schema: &SecondarySchemaPlan<'_>, options: &CodegenOptions) -> String {
    let mut body = String::new();

    for enum_plan in &schema.section.enums {
        body.push_str(&render_enum(enum_plan));
    }

    for table in &schema.section.tables {
        body.push_str(&render_record(table, options));
    }

    // Nothing is declared inside the module, so the block is left out entirely
    // instead of carrying an import no item needs.
    if body.is_empty() {
        return body;
    }

    let mut out = String::new();

    out.push_str("// The module owns its own type namespace, so the prelude its items\n");
    out.push_str("// are written against is imported here rather than inherited.\n");
    out.push_str("#[allow(unused_imports)]\n");
    out.push_str("use rurqe::prelude::*;\n\n");
    out.push_str(&body);

    out
}

/// Renders one `#[derive(SqlType)]` enum definition.
fn render_enum(enum_plan: &EnumPlan<'_>) -> String {
    let mut out = String::new();

    out.push_str("/// The `");
    out.push_str(&doc_text(enum_plan.name));
    out.push_str("` enum.\n");
    out.push_str("#[derive(SqlType, Clone, Copy, PartialEq, Eq, Debug)]\n");
    out.push_str("pub enum ");
    out.push_str(&enum_plan.ident);
    out.push_str(" {\n");

    for variant in &enum_plan.variants {
        if let Some(ref rename) = variant.rename {
            out.push_str("    #[sql(rename = \"");
            out.push_str(&escape_literal(rename));
            out.push_str("\")]\n");
        }
        out.push_str("    ");
        out.push_str(&variant.ident);
        out.push_str(",\n");
    }

    out.push_str("}\n\n");

    out
}

/// Renders the `table!` invocation declaring one table and its column markers.
fn render_table_block(table: &TablePlan<'_>) -> String {
    let mut out = String::new();

    out.push_str("table! {\n");
    out.push_str("    /// The `");
    out.push_str(&doc_text(table.name));
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

    out
}

/// Renders the row record of one table together with its `FromRow` implementation.
///
/// The record is omitted when record generation is disabled, so the caller can
/// ask for it unconditionally.
fn render_record(table: &TablePlan<'_>, options: &CodegenOptions) -> String {
    if !options.generate_records {
        return String::new();
    }

    let mut out = String::new();

    out.push_str("/// Strongly typed row record for `");
    out.push_str(&doc_text(table.name));
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
        out.push_str(": FromSqlValue::from_sql_value(\n");
        out.push_str("                values\n");
        out.push_str("                    .get(");
        out.push_str(&index.to_string());
        out.push_str(")\n");
        out.push_str("                    .ok_or(RowDecodeError::ArityMismatch {\n");
        out.push_str("                        expected: ");
        out.push_str(&arity);
        out.push_str(",\n");
        out.push_str("                        actual: values.len(),\n");
        out.push_str("                    })?,\n");
        out.push_str("            )?,\n");
    }

    out.push_str("        })\n");
    out.push_str("    }\n");
    out.push_str("}\n\n");

    out
}

/// Appends `block` to `out`, prefixing every non-empty line with `indent`.
fn push_block(out: &mut String, block: &str, indent: &str) {
    for line in block.lines() {
        if line.is_empty() {
            out.push('\n');
        } else {
            out.push_str(indent);
            out.push_str(line);
            out.push('\n');
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rurqe_migrate::catalog::{ColumnDef, EnumDef, TableDef, TypeKind};

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
        assert!(code.contains("fn try_from_row(values: &[SqlVal]) -> Result<Self, RowDecodeError>"));
        assert!(code.contains("RowDecodeError::ArityMismatch"));
        // The generated row reader never indexes the slice directly, so a short
        // response reports `ArityMismatch` instead of panicking.
        assert!(code.contains("values\n                    .get(0)"));
        assert!(!code.contains("values["));
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
    fn doc_comments_never_break_out_of_the_comment() {
        let catalog = SchemaCatalog::new(vec![TableDef::new(
            "users\npub struct Injected;",
            vec![ColumnDef::new("id", TypeKind::Int64, false)],
            Vec::new(),
        )])
        .with_enums(vec![EnumDef::new(
            "mood\r\nfn injected() {}",
            vec!["ok".to_owned()],
        )]);

        let code =
            generate_schema_rs(&catalog, &CodegenOptions::default()).expect("codegen succeeds");

        // The names reach the comments on the line they were written on.
        assert!(code.contains("/// The `users\\npub struct Injected;` table."));
        assert!(code.contains("/// The `mood\\r\\nfn injected() {}` enum."));
        assert!(code.contains("/// Strongly typed row record for `users\\npub struct Injected;`."));

        // No control character reaches the generated module, so a name can never
        // end the comment it was rendered into nor start a line of its own.
        assert!(!code.contains('\r'));

        for line in code.lines() {
            let trimmed = line.trim_start();
            if trimmed.starts_with("//") {
                continue;
            }
            assert!(!trimmed.starts_with("fn injected"), "{line}");
            assert!(!trimmed.starts_with("pub struct Injected"), "{line}");
        }

        // The names reach the module through their sanitized identifiers, so it
        // compiles with exactly the items the generator declares.
        assert!(code.contains("UsersPubStructInjected => \"users\\npub struct Injected;\""));
        assert!(code.contains("pub enum MoodFnInjected {"));
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
    /// The generated module with its header and schema hash removed.
    fn body_after_hash(code: &str) -> &str {
        let hash = code
            .find("pub const SCHEMA_HASH")
            .expect("the module declares the hash");
        let offset = code[hash..]
            .find("\n\n")
            .expect("the hash constant is its own paragraph");

        &code[hash + offset + 2..]
    }

    #[test]
    fn secondary_schemas_render_as_schema_blocks() {
        let catalog = SchemaCatalog::new(vec![
            TableDef::new(
                "users",
                vec![
                    ColumnDef::new("id", TypeKind::Int64, false),
                    ColumnDef::new("email", TypeKind::Text, true),
                ],
                vec!["id".to_owned()],
            ),
            TableDef::new(
                "sessions",
                vec![ColumnDef::new("id", TypeKind::Int64, false)],
                Vec::new(),
            )
            .with_schema("auth"),
        ])
        .with_enums(vec![
            EnumDef::new("mood", vec!["happy".to_owned()]).with_schema("auth"),
        ]);

        let code =
            generate_schema_rs(&catalog, &CodegenOptions::default()).expect("codegen succeeds");

        // The primary table stays a flat item of the module.
        assert!(code.contains("table! {\n    /// The `users` table.\n    Users => \"users\" {"));

        // The secondary schema declares the marker it is addressed through next
        // to the module holding its type namespace.
        assert!(code.contains("schema! {\n    /// The `auth` schema.\n    Auth => \"auth\" as auth {\n"));
        assert!(code.contains(
            "        table! {\n            /// The `sessions` table.\n            Sessions => \"sessions\" {\n                SessionsId => \"id\": i64,\n            }\n        }\n"
        ));
        assert!(code.contains("        @items {\n"));

        // The items of the schema land in its module, where the prelude they are
        // written against is imported.
        assert!(code.contains("            use rurqe::prelude::*;\n"));
        // The label `happy` is not the variant identifier `Happy`, so the
        // rendered variant keeps the stored label through the rename attribute.
        assert!(code.contains(
            "            pub enum Mood {\n                #[sql(rename = \"happy\")]\n                Happy,\n            }\n"
        ));
        assert!(code.contains("            pub struct SessionsRecord {\n                pub id: i64,\n            }\n"));
        assert!(code.contains("            impl FromRow for SessionsRecord {\n"));
    }

    #[test]
    fn the_primary_section_renders_exactly_as_a_flat_catalog() {
        let flat_tables = vec![
            TableDef::new(
                "users",
                vec![
                    ColumnDef::new("id", TypeKind::Int64, false),
                    ColumnDef::new("email", TypeKind::Text, true),
                ],
                vec!["id".to_owned()],
            ),
            TableDef::new(
                "orders",
                vec![ColumnDef::new("total", TypeKind::Float64, false)],
                Vec::new(),
            ),
        ];

        let flat = generate_schema_rs(
            &SchemaCatalog::new(flat_tables.clone()),
            &CodegenOptions::default(),
        )
        .expect("codegen succeeds");

        let mut mixed_tables = flat_tables;
        mixed_tables.push(
            TableDef::new(
                "sessions",
                vec![ColumnDef::new("id", TypeKind::Int64, false)],
                Vec::new(),
            )
            .with_schema("auth"),
        );

        let mixed = generate_schema_rs(
            &SchemaCatalog::new(mixed_tables),
            &CodegenOptions::default(),
        )
        .expect("codegen succeeds");

        let flat_body = body_after_hash(&flat);
        let mixed_body = body_after_hash(&mixed);

        assert!(
            mixed_body.starts_with(flat_body),
            "the flat primary section changed:\n{mixed_body}"
        );
        assert!(mixed_body[flat_body.len()..].starts_with("schema! {\n"));
    }
}
