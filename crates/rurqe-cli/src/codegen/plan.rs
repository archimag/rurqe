//! Assignment of Rust identifiers to every generated schema item.
//!
//! The plan is built before any text is rendered, so two schema names that
//! reduce to the same identifier are reported as
//! [`CodegenError::IdentifierCollision`] instead of emitting a module with
//! duplicate definitions.

use std::collections::BTreeMap;

use rurqe_migrate::catalog::{ColumnDef, EnumDef, SchemaCatalog, TableDef, TypeKind};

use super::naming::{table_type_ident, to_pascal_case, to_safe_ident};
use super::{CodegenError, CodegenOptions};

/// The full generation plan for a schema.
#[derive(Debug)]
pub(super) struct SchemaPlan<'a> {
    pub(super) enums: Vec<EnumPlan<'a>>,
    pub(super) tables: Vec<TablePlan<'a>>,
}

/// An enum rendered into its Rust identifiers and variants.
#[derive(Debug)]
pub(super) struct EnumPlan<'a> {
    pub(super) name: &'a str,
    pub(super) ident: String,
    pub(super) variants: Vec<EnumVariantPlan>,
}

/// A variant of an enum.
#[derive(Debug)]
pub(super) struct EnumVariantPlan {
    pub(super) ident: String,
    pub(super) rename: Option<String>,
}

/// A table rendered into its Rust identifiers.
#[derive(Debug)]
pub(super) struct TablePlan<'a> {
    pub(super) name: &'a str,
    pub(super) ident: String,
    pub(super) record_ident: String,
    pub(super) columns: Vec<ColumnPlan<'a>>,
}

/// A column rendered into its Rust identifiers and value type.
#[derive(Debug)]
pub(super) struct ColumnPlan<'a> {
    pub(super) name: &'a str,
    pub(super) marker_ident: String,
    pub(super) field_ident: String,
    pub(super) rust_type: String,
}

/// Assigns Rust identifiers to every generated item and rejects collisions.
pub(super) fn build_plan<'a>(
    catalog: &'a SchemaCatalog,
    options: &CodegenOptions,
) -> Result<SchemaPlan<'a>, CodegenError> {
    // The type namespace is global: enum types, table markers, column markers, and records
    // share one module, so every generated type name must be unique.
    let mut types: BTreeMap<String, Vec<String>> = BTreeMap::new();

    let mut enums: Vec<&'a EnumDef> = catalog.enums.iter().collect();
    enums.sort_by(|a, b| a.name.cmp(&b.name));

    let mut enum_plans = Vec::with_capacity(enums.len());
    for enum_def in enums {
        let ident = table_type_ident(&enum_def.name);
        claim(&mut types, &ident, format!("enum `{}`", enum_def.name));

        let mut variant_plans = Vec::with_capacity(enum_def.variants.len());
        let mut variant_idents: BTreeMap<String, Vec<String>> = BTreeMap::new();

        for variant in &enum_def.variants {
            let variant_ident = table_type_ident(variant);
            claim(
                &mut variant_idents,
                &variant_ident,
                format!("variant `{variant}` of enum `{}`", enum_def.name),
            );

            let rename = if variant != &variant_ident {
                Some(variant.clone())
            } else {
                None
            };

            variant_plans.push(EnumVariantPlan {
                ident: variant_ident,
                rename,
            });
        }

        if let Some((ident, sources)) = first_collision(&variant_idents) {
            return Err(CodegenError::IdentifierCollision { ident, sources });
        }

        enum_plans.push(EnumPlan {
            name: enum_def.name.as_str(),
            ident,
            variants: variant_plans,
        });
    }

    let mut tables: Vec<&'a TableDef> = catalog.tables.iter().collect();
    tables.sort_by(|a, b| a.name.cmp(&b.name));

    let mut plan = Vec::with_capacity(tables.len());

    for table in tables {
        let ident = table_type_ident(&table.name);
        claim(&mut types, &ident, format!("table `{}`", table.name));

        let record_ident = format!("{ident}Record");
        if options.generate_records {
            claim(
                &mut types,
                &record_ident,
                format!("record of table `{}`", table.name),
            );
        }

        let mut columns: Vec<&'a ColumnDef> = table.columns.iter().collect();
        columns.sort_by(|a, b| a.name.cmp(&b.name));

        // Record fields live in a per-table namespace: two columns of one table
        // may not share a field name, while equal field names across tables are
        // harmless because each record is a separate struct.
        let mut fields: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let mut column_plans = Vec::with_capacity(columns.len());

        for column in columns {
            let source = format!("column `{}.{}`", table.name, column.name);

            let marker_ident = format!("{ident}{}", to_pascal_case(&column.name));
            claim(&mut types, &marker_ident, source.clone());

            let field_ident = to_safe_ident(&column.name);
            claim(&mut fields, &field_ident, source);

            column_plans.push(ColumnPlan {
                name: column.name.as_str(),
                marker_ident,
                field_ident,
                rust_type: column_rust_type(table.name.as_str(), column, options),
            });
        }

        if let Some((ident, sources)) = first_collision(&fields) {
            return Err(CodegenError::IdentifierCollision { ident, sources });
        }

        plan.push(TablePlan {
            name: table.name.as_str(),
            ident,
            record_ident,
            columns: column_plans,
        });
    }

    if let Some((ident, sources)) = first_collision(&types) {
        return Err(CodegenError::IdentifierCollision { ident, sources });
    }

    Ok(SchemaPlan {
        enums: enum_plans,
        tables: plan,
    })
}

/// Registers `ident` as produced by `source`, so later claims can detect collisions.
fn claim(namespace: &mut BTreeMap<String, Vec<String>>, ident: &str, source: String) {
    namespace.entry(ident.to_owned()).or_default().push(source);
}

/// Returns the first identifier claimed by more than one source, in sorted order.
fn first_collision(namespace: &BTreeMap<String, Vec<String>>) -> Option<(String, Vec<String>)> {
    namespace
        .iter()
        .find(|(_, sources)| sources.len() > 1)
        .map(|(ident, sources)| (ident.clone(), sources.clone()))
}

/// Resolves the Rust type generated for `column`, honouring project overrides.
///
/// An override replaces the mapped type entirely and keeps the column's
/// nullability, so `uuid::Uuid` stays `uuid::Uuid` for a `NOT NULL` column and
/// becomes `Option<uuid::Uuid>` for a nullable one.
fn column_rust_type(table: &str, column: &ColumnDef, options: &CodegenOptions) -> String {
    let path = format!("{table}.{}", column.name);

    match options
        .type_overrides
        .get(&path)
        .map(|rust_type| rust_type.trim())
    {
        Some(rust_type) if !rust_type.is_empty() => apply_nullability(rust_type, column.nullable),
        _ => type_kind_to_rust_type(&column.type_kind, column.nullable, options.domain_types),
    }
}

/// Wraps `rust_type` into `Option<..>` for a nullable column unless it already
/// spells an optional type, so an override is never wrapped twice.
fn apply_nullability(rust_type: &str, nullable: bool) -> String {
    if nullable && !rust_type.starts_with("Option<") {
        format!("Option<{rust_type}>")
    } else {
        rust_type.to_owned()
    }
}

/// Maps a catalog type onto the Rust type carried by the generated markers.
///
/// Integer width follows the catalog: SQLite reports every integer column as
/// `Int64`, while PostgreSQL distinguishes `Int32` from `Int64`, so an `i32`
/// column is never widened silently.
///
/// An array column is represented as `Vec<..>` of its element type, and an enum
/// column as the element type it is declared over.
///
/// `Uuid`, `Json`, `Timestamp`, `Date` and engine-specific `Other` columns are
/// represented as `String` by default, because the generated module then compiles
/// without extra dependencies. With [`CodegenOptions::domain_types`] enabled they
/// map onto `uuid::Uuid`, `serde_json::Value`, `chrono::NaiveDateTime`, and
/// `chrono::NaiveDate` instead.
fn type_kind_to_rust_type(kind: &TypeKind, nullable: bool, domain_types: bool) -> String {
    if let TypeKind::Array(inner) = kind {
        let element = type_kind_to_rust_type(inner, false, domain_types);
        let base = format!("Vec<{element}>");
        return if nullable {
            format!("Option<{base}>")
        } else {
            base
        };
    }

    let base = match kind {
        TypeKind::Boolean => "bool".to_owned(),
        TypeKind::Int32 => "i32".to_owned(),
        TypeKind::Int64 => "i64".to_owned(),
        TypeKind::Float64 => "f64".to_owned(),
        TypeKind::Text => "String".to_owned(),
        TypeKind::Bytes => "Vec<u8>".to_owned(),
        TypeKind::Uuid if domain_types => "uuid::Uuid".to_owned(),
        TypeKind::Json if domain_types => "serde_json::Value".to_owned(),
        TypeKind::Timestamp if domain_types => "chrono::NaiveDateTime".to_owned(),
        TypeKind::Date if domain_types => "chrono::NaiveDate".to_owned(),
        TypeKind::Uuid | TypeKind::Json | TypeKind::Timestamp | TypeKind::Date => "String".to_owned(),
        TypeKind::Enum(name) => table_type_ident(name),
        TypeKind::Other(_) => "String".to_owned(),
        TypeKind::Array(_) => "String".to_owned(),
    };

    if nullable {
        format!("Option<{base}>")
    } else {
        base
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table(name: &str, columns: Vec<ColumnDef>) -> TableDef {
        TableDef::new(name, columns, Vec::new())
    }

    fn rust_type_of(column: &ColumnDef, options: &CodegenOptions) -> String {
        column_rust_type("users", column, options)
    }

    #[test]
    fn colliding_field_identifiers_are_rejected() {
        let catalog = SchemaCatalog::new(vec![table(
            "users",
            vec![
                ColumnDef::new("user id", TypeKind::Int64, false),
                ColumnDef::new("user_id", TypeKind::Int64, false),
            ],
        )]);

        let error = build_plan(&catalog, &CodegenOptions::default())
            .expect_err("equal field identifiers are rejected");

        match &error {
            CodegenError::IdentifierCollision { ident, sources } => {
                assert_eq!(ident, "user_id");
                assert_eq!(
                    sources,
                    &vec![
                        "column `users.user id`".to_owned(),
                        "column `users.user_id`".to_owned(),
                    ]
                );
            }
        }
        assert!(error.to_string().contains("user_id"));
        assert!(error.to_string().contains("collide"));
    }

    #[test]
    fn colliding_type_identifiers_are_rejected() {
        // The `record` column marker `UsersRecord` clashes with the generated
        // `UsersRecord` row record of the same table.
        let catalog = SchemaCatalog::new(vec![table(
            "users",
            vec![ColumnDef::new("record", TypeKind::Text, false)],
        )]);

        let error = build_plan(&catalog, &CodegenOptions::default())
            .expect_err("type identifier collisions are rejected");

        match &error {
            CodegenError::IdentifierCollision { ident, sources } => {
                assert_eq!(ident, "UsersRecord");
                assert_eq!(sources.len(), 2);
            }
        }
    }

    #[test]
    fn colliding_table_identifiers_are_rejected() {
        let catalog = SchemaCatalog::new(vec![
            table(
                "user_profiles",
                vec![ColumnDef::new("id", TypeKind::Int64, false)],
            ),
            table(
                "user.profiles",
                vec![ColumnDef::new("id", TypeKind::Int64, false)],
            ),
        ]);

        let error = build_plan(&catalog, &CodegenOptions::default())
            .expect_err("colliding table identifiers are rejected");

        match &error {
            CodegenError::IdentifierCollision { ident, .. } => {
                assert_eq!(ident, "UserProfiles");
            }
        }
    }

    #[test]
    fn colliding_enum_identifiers_are_rejected() {
        let catalog = SchemaCatalog::new(Vec::new()).with_enums(vec![
            EnumDef::new("post_status", vec!["active".to_owned()]),
            EnumDef::new("post-status", vec!["active".to_owned()]),
        ]);

        let error = build_plan(&catalog, &CodegenOptions::default())
            .expect_err("colliding enum identifiers are rejected");

        match &error {
            CodegenError::IdentifierCollision { ident, .. } => {
                assert_eq!(ident, "PostStatus");
            }
        }
    }

    #[test]
    fn type_overrides_replace_the_default_mapping() {
        let mut options = CodegenOptions::default();
        options
            .type_overrides
            .insert("users.id".to_owned(), "uuid::Uuid".to_owned());
        options.type_overrides.insert(
            "users.created_at".to_owned(),
            "chrono::NaiveDateTime".to_owned(),
        );

        assert_eq!(
            rust_type_of(&ColumnDef::new("id", TypeKind::Uuid, false), &options),
            "uuid::Uuid"
        );
        assert_eq!(
            rust_type_of(&ColumnDef::new("created_at", TypeKind::Timestamp, true), &options),
            "Option<chrono::NaiveDateTime>"
        );
    }

    #[test]
    fn type_overrides_respect_declared_nullability() {
        let mut options = CodegenOptions::default();
        options
            .type_overrides
            .insert("users.id".to_owned(), "Option<uuid::Uuid>".to_owned());
        options.type_overrides.insert(
            "users.deleted_at".to_owned(),
            "Option<chrono::NaiveDateTime>".to_owned(),
        );

        // A `NOT NULL` column keeps the type exactly as written ...
        assert_eq!(
            rust_type_of(&ColumnDef::new("id", TypeKind::Int64, false), &options),
            "Option<uuid::Uuid>"
        );
        // ... and an already optional override is never wrapped twice.
        assert_eq!(
            rust_type_of(&ColumnDef::new("deleted_at", TypeKind::Timestamp, true), &options),
            "Option<chrono::NaiveDateTime>"
        );
    }

    #[test]
    fn columns_without_an_override_keep_the_default_mapping() {
        let mut options = CodegenOptions::default();
        options
            .type_overrides
            .insert("orders.total".to_owned(), "Money".to_owned());

        assert_eq!(
            rust_type_of(&ColumnDef::new("email", TypeKind::Text, false), &options),
            "String"
        );
    }

    #[test]
    fn domain_types_map_uuid_json_and_temporal_columns() {
        let options = CodegenOptions {
            domain_types: true,
            ..CodegenOptions::default()
        };

        assert_eq!(
            rust_type_of(&ColumnDef::new("id", TypeKind::Uuid, false), &options),
            "uuid::Uuid"
        );
        assert_eq!(
            rust_type_of(&ColumnDef::new("payload", TypeKind::Json, true), &options),
            "Option<serde_json::Value>"
        );
        assert_eq!(
            rust_type_of(&ColumnDef::new("created_at", TypeKind::Timestamp, false), &options),
            "chrono::NaiveDateTime"
        );
        assert_eq!(
            rust_type_of(&ColumnDef::new("on_day", TypeKind::Date, true), &options),
            "Option<chrono::NaiveDate>"
        );
    }

    #[test]
    fn domain_types_are_opt_in() {
        let options = CodegenOptions::default();

        assert_eq!(
            rust_type_of(&ColumnDef::new("id", TypeKind::Uuid, false), &options),
            "String"
        );
        assert_eq!(
            rust_type_of(&ColumnDef::new("created_at", TypeKind::Timestamp, true), &options),
            "Option<String>"
        );
    }

    #[test]
    fn domain_types_still_yield_to_column_overrides() {
        let mut options = CodegenOptions {
            domain_types: true,
            ..CodegenOptions::default()
        };
        options
            .type_overrides
            .insert("users.id".to_owned(), "EventId".to_owned());

        assert_eq!(
            rust_type_of(&ColumnDef::new("id", TypeKind::Uuid, false), &options),
            "EventId"
        );
    }

    #[test]
    fn enum_columns_map_to_their_type_identifiers() {
        assert_eq!(
            rust_type_of(
                &ColumnDef::new("mood", TypeKind::Enum("user_mood".to_owned()), false),
                &CodegenOptions::default(),
            ),
            "UserMood"
        );
        assert_eq!(
            rust_type_of(
                &ColumnDef::new("mood", TypeKind::Enum("user_mood".to_owned()), true),
                &CodegenOptions::default(),
            ),
            "Option<UserMood>"
        );
    }

    #[test]
    fn array_columns_map_to_vectors_of_their_element() {
        let options = CodegenOptions::default();

        assert_eq!(
            rust_type_of(
                &ColumnDef::new("tags", TypeKind::Array(Box::new(TypeKind::Text)), false),
                &options,
            ),
            "Vec<String>"
        );
        assert_eq!(
            rust_type_of(
                &ColumnDef::new("moods", TypeKind::Array(Box::new(TypeKind::Enum(
                    "user_mood".to_owned()
                ))), true),
                &options,
            ),
            "Option<Vec<UserMood>>"
        );
    }
}
