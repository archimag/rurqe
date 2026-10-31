//! Assignment of Rust identifiers to every generated schema item.
//!
//! The plan is partitioned by database schema. The entities of the primary
//! schema, the ones carrying no attribution, stay flat items of the generated
//! module, so a query addresses them through the bare markers exactly as a
//! single-schema catalog does. The entities of every secondary schema are
//! grouped into a section rendered as a `schema!` block, and each section owns
//! its type namespace while the marker and the module of a secondary schema are
//! claimed at the top level.
//!
//! The plan is built before any text is rendered, so two names that reduce to
//! the same identifier are reported as [`CodegenError::IdentifierCollision`]
//! instead of emitting a module with duplicate definitions.

use std::collections::BTreeMap;

use rurqe_migrate::catalog::{ColumnDef, EnumDef, SchemaCatalog, TableDef, TypeKind};

use super::naming::{table_type_ident, to_pascal_case, to_safe_ident};
use super::{CodegenError, CodegenOptions};

/// The full generation plan for a schema.
#[derive(Debug)]
pub(super) struct SchemaPlan<'a> {
    /// The primary schema, rendered flat at the top level of the module.
    pub(super) primary: SectionPlan<'a>,
    /// The secondary schemas, in canonical name order.
    pub(super) secondary: Vec<SecondarySchemaPlan<'a>>,
}

/// The items of one schema section.
#[derive(Debug)]
pub(super) struct SectionPlan<'a> {
    pub(super) enums: Vec<EnumPlan<'a>>,
    pub(super) tables: Vec<TablePlan<'a>>,
}

/// A secondary schema together with the identifiers it claims at the top level.
#[derive(Debug)]
pub(super) struct SecondarySchemaPlan<'a> {
    /// The SQL name of the schema.
    pub(super) name: &'a str,
    /// The zero-sized marker every table of the schema is addressed through.
    pub(super) ident: String,
    /// The module holding the type namespace of the schema.
    pub(super) module: String,
    /// The items declared inside [`SecondarySchemaPlan::module`].
    pub(super) section: SectionPlan<'a>,
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
    // The type namespace of the top level: the flat items of the primary schema
    // share it with the marker and the module of every secondary schema, because
    // a schema is addressed from the very same module level.
    let mut top: BTreeMap<String, Vec<String>> = BTreeMap::new();

    let mut primary_enums: Vec<&'a EnumDef> = Vec::new();
    let mut secondary_enums: BTreeMap<&'a str, Vec<&'a EnumDef>> = BTreeMap::new();
    for enum_def in &catalog.enums {
        match enum_def.schema.as_deref() {
            Some(schema) => secondary_enums.entry(schema).or_default().push(enum_def),
            None => primary_enums.push(enum_def),
        }
    }

    let mut primary_tables: Vec<&'a TableDef> = Vec::new();
    let mut secondary_tables: BTreeMap<&'a str, Vec<&'a TableDef>> = BTreeMap::new();
    for table in &catalog.tables {
        match table.schema.as_deref() {
            Some(schema) => secondary_tables.entry(schema).or_default().push(table),
            None => primary_tables.push(table),
        }
    }

    // The primary section is rendered flat, so its items claim the top-level
    // namespace directly and keep the collision rules a single-schema catalog
    // already had.
    let primary = build_section(&mut top, primary_enums, primary_tables, None, options)?;

    let mut names: Vec<&'a str> = secondary_tables
        .keys()
        .copied()
        .chain(secondary_enums.keys().copied())
        .collect();
    names.sort_unstable();
    names.dedup();

    let mut secondary = Vec::with_capacity(names.len());

    for name in names {
        let ident = table_type_ident(name);
        claim(&mut top, &ident, format!("schema `{name}`"));

        let module = to_safe_ident(name);
        claim(&mut top, &module, format!("module of schema `{name}`"));

        let mut namespace: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let section = build_section(
            &mut namespace,
            secondary_enums.remove(name).unwrap_or_default(),
            secondary_tables.remove(name).unwrap_or_default(),
            Some(name),
            options,
        )?;

        // The section owns its type namespace, so the collision rules apply
        // within it exactly as they apply to the flat primary section.
        if let Some((ident, sources)) = first_collision(&namespace) {
            return Err(CodegenError::IdentifierCollision { ident, sources });
        }

        secondary.push(SecondarySchemaPlan {
            name,
            ident,
            module,
            section,
        });
    }

    if let Some((ident, sources)) = first_collision(&top) {
        return Err(CodegenError::IdentifierCollision { ident, sources });
    }

    Ok(SchemaPlan { primary, secondary })
}

/// Assigns the identifiers of one schema section into its own `namespace`.
///
/// The caller decides which namespace the section claims: the primary section
/// claims the top-level one, while every secondary schema claims a namespace of
/// its own, which is what keeps equal table names of two schemas apart.
fn build_section<'a>(
    namespace: &mut BTreeMap<String, Vec<String>>,
    mut enums: Vec<&'a EnumDef>,
    mut tables: Vec<&'a TableDef>,
    schema: Option<&str>,
    options: &CodegenOptions,
) -> Result<SectionPlan<'a>, CodegenError> {
    enums.sort_by(|a, b| a.name.cmp(&b.name));

    let mut enum_plans = Vec::with_capacity(enums.len());
    for enum_def in enums {
        let ident = table_type_ident(&enum_def.name);
        claim(namespace, &ident, format!("enum `{}`", enum_def.name));

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

    tables.sort_by(|a, b| a.name.cmp(&b.name));

    let mut table_plans = Vec::with_capacity(tables.len());

    for table in tables {
        let ident = table_type_ident(&table.name);
        claim(namespace, &ident, format!("table `{}`", table.name));

        let record_ident = format!("{ident}Record");
        if options.generate_records {
            claim(
                namespace,
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
            claim(namespace, &marker_ident, source.clone());

            let field_ident = to_safe_ident(&column.name);
            claim(&mut fields, &field_ident, source);

            column_plans.push(ColumnPlan {
                name: column.name.as_str(),
                marker_ident,
                field_ident,
                rust_type: column_rust_type(schema, table.name.as_str(), column, options),
            });
        }

        if let Some((ident, sources)) = first_collision(&fields) {
            return Err(CodegenError::IdentifierCollision { ident, sources });
        }

        table_plans.push(TablePlan {
            name: table.name.as_str(),
            ident,
            record_ident,
            columns: column_plans,
        });
    }

    Ok(SectionPlan {
        enums: enum_plans,
        tables: table_plans,
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
/// A column of the primary schema is overridden through its `table.column`
/// path, while a column of a secondary schema is overridden through the
/// `schema.table.column` path the configuration file produces for it.
///
/// An override replaces the mapped type entirely and keeps the column's
/// nullability, so `uuid::Uuid` stays `uuid::Uuid` for a `NOT NULL` column and
/// becomes `Option<uuid::Uuid>` for a nullable one.
fn column_rust_type(
    schema: Option<&str>,
    table: &str,
    column: &ColumnDef,
    options: &CodegenOptions,
) -> String {
    let path = match schema {
        Some(schema) => format!("{schema}.{table}.{}", column.name),
        None => format!("{table}.{}", column.name),
    };

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
    if nullable && !is_optional_type(rust_type) {
        format!("Option<{rust_type}>")
    } else {
        rust_type.to_owned()
    }
}

/// Reports whether `rust_type` spells an optional type.
///
/// The override is parsed as a Rust type instead of being matched textually, so
/// every spelling of `Option` — `Option<T>`, `std::option::Option<T>`, or
/// `::core::option::Option<T>` — is recognised and the column type is never
/// wrapped twice. A type the parser rejects stays as written, because an invalid
/// override is trusted configuration that fails to compile on its own.
fn is_optional_type(rust_type: &str) -> bool {
    match syn::parse_str::<syn::Type>(rust_type) {
        Ok(ty) => optional_path(&ty),
        Err(_) => false,
    }
}

/// Whether a parsed Rust type is an optional wrapper spelled through a path.
fn optional_path(ty: &syn::Type) -> bool {
    match ty {
        syn::Type::Path(path) => path
            .path
            .segments
            .last()
            .is_some_and(|segment| segment.ident == "Option" && !segment.arguments.is_empty()),
        syn::Type::Paren(paren) => optional_path(&paren.elem),
        syn::Type::Group(group) => optional_path(&group.elem),
        _ => false,
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
/// `Uuid`, `Json`, `Timestamp`, `Date`, `Numeric` and engine-specific `Other`
/// columns are represented as `String` by default, because the generated module
/// then compiles without extra dependencies. With
/// [`CodegenOptions::domain_types`] enabled they map onto `uuid::Uuid`,
/// `serde_json::Value`, `chrono::NaiveDateTime`, `chrono::NaiveDate`, and
/// `rust_decimal::Decimal` instead.
fn type_kind_to_rust_type(kind: &TypeKind, nullable: bool, domain_types: bool) -> String {
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
        TypeKind::Numeric if domain_types => "rust_decimal::Decimal".to_owned(),
        TypeKind::Uuid
        | TypeKind::Json
        | TypeKind::Timestamp
        | TypeKind::Date
        | TypeKind::Numeric => "String".to_owned(),
        TypeKind::Enum(name) => table_type_ident(name),
        TypeKind::Other(_) => "String".to_owned(),
        // The element keeps its own nullability out of the picture: a nullable array
        // column wraps the vector, never its elements, so the element is mapped as
        // non-nullable and only the shared trailing wrapper below applies.
        TypeKind::Array(inner) => {
            format!(
                "Vec<{}>",
                type_kind_to_rust_type(inner, false, domain_types)
            )
        }
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
        column_rust_type(None, "users", column, options)
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
            rust_type_of(
                &ColumnDef::new("created_at", TypeKind::Timestamp, true),
                &options
            ),
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
            rust_type_of(
                &ColumnDef::new("deleted_at", TypeKind::Timestamp, true),
                &options
            ),
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
            rust_type_of(
                &ColumnDef::new("created_at", TypeKind::Timestamp, false),
                &options
            ),
            "chrono::NaiveDateTime"
        );
        assert_eq!(
            rust_type_of(&ColumnDef::new("on_day", TypeKind::Date, true), &options),
            "Option<chrono::NaiveDate>"
        );
        assert_eq!(
            rust_type_of(&ColumnDef::new("total", TypeKind::Numeric, false), &options),
            "rust_decimal::Decimal"
        );
        assert_eq!(
            rust_type_of(
                &ColumnDef::new("discount", TypeKind::Numeric, true),
                &options
            ),
            "Option<rust_decimal::Decimal>"
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
            rust_type_of(
                &ColumnDef::new("created_at", TypeKind::Timestamp, true),
                &options
            ),
            "Option<String>"
        );
        assert_eq!(
            rust_type_of(&ColumnDef::new("total", TypeKind::Numeric, true), &options),
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
                &ColumnDef::new(
                    "moods",
                    TypeKind::Array(Box::new(TypeKind::Enum("user_mood".to_owned()))),
                    true
                ),
                &options,
            ),
            "Option<Vec<UserMood>>"
        );
    }

    #[test]
    fn a_qualified_optional_override_is_not_wrapped_twice() {
        let mut options = CodegenOptions::default();
        options.type_overrides.insert(
            "users.id".to_owned(),
            "std::option::Option<uuid::Uuid>".to_owned(),
        );
        options.type_overrides.insert(
            "users.deleted_at".to_owned(),
            "core::option::Option<chrono::NaiveDateTime>".to_owned(),
        );

        assert_eq!(
            rust_type_of(&ColumnDef::new("id", TypeKind::Int64, true), &options),
            "std::option::Option<uuid::Uuid>"
        );
        assert_eq!(
            rust_type_of(
                &ColumnDef::new("deleted_at", TypeKind::Timestamp, true),
                &options
            ),
            "core::option::Option<chrono::NaiveDateTime>"
        );
    }
    #[test]
    fn a_secondary_schema_keeps_its_own_type_namespace() {
        // Both sections declare the very same table name, so the markers are
        // equal; only the section they are registered in keeps them apart.
        let catalog = SchemaCatalog::new(vec![
            table(
                "users",
                vec![ColumnDef::new("id", TypeKind::Int64, false)],
            ),
            table(
                "users",
                vec![ColumnDef::new("id", TypeKind::Int64, false)],
            )
            .with_schema("auth"),
        ]);

        let plan = build_plan(&catalog, &CodegenOptions::default())
            .expect("sections own their namespaces, so equal markers never collide");

        assert_eq!(plan.primary.tables.len(), 1);
        assert_eq!(plan.primary.tables[0].ident, "Users");

        assert_eq!(plan.secondary.len(), 1);
        let auth = &plan.secondary[0];
        assert_eq!(auth.name, "auth");
        assert_eq!(auth.ident, "Auth");
        assert_eq!(auth.module, "auth");
        assert_eq!(auth.section.tables.len(), 1);
        assert_eq!(auth.section.tables[0].ident, "Users");
    }

    #[test]
    fn a_secondary_column_is_overridden_through_its_schema_path() {
        let mut options = CodegenOptions::default();
        options
            .type_overrides
            .insert("auth.users.id".to_owned(), "uuid::Uuid".to_owned());
        options
            .type_overrides
            .insert("users.email".to_owned(), "Email".to_owned());

        let id = ColumnDef::new("id", TypeKind::Int64, false);
        assert_eq!(
            column_rust_type(Some("auth"), "users", &id, &options),
            "uuid::Uuid"
        );
        // A two-segment path names a column of the primary schema, so it never
        // reaches the column of a secondary one.
        assert_eq!(column_rust_type(None, "users", &id, &options), "i64");

        let email = ColumnDef::new("email", TypeKind::Text, false);
        assert_eq!(
            column_rust_type(Some("auth"), "users", &email, &options),
            "String"
        );
        assert_eq!(column_rust_type(None, "users", &email, &options), "Email");
    }

    #[test]
    fn a_schema_marker_colliding_with_a_primary_table_is_rejected() {
        let catalog = SchemaCatalog::new(vec![
            table("auth", vec![ColumnDef::new("id", TypeKind::Int64, false)]),
            table("users", vec![ColumnDef::new("id", TypeKind::Int64, false)]).with_schema("auth"),
        ]);

        let error = build_plan(&catalog, &CodegenOptions::default())
            .expect_err("a schema marker colliding with a primary table is rejected");

        match &error {
            CodegenError::IdentifierCollision { ident, sources } => {
                assert_eq!(ident, "Auth");
                assert_eq!(
                    sources,
                    &vec!["table `auth`".to_owned(), "schema `auth`".to_owned()]
                );
            }
        }
        assert!(error.to_string().contains("Auth"));
        assert!(error.to_string().contains("schema `auth`"));
    }
}
