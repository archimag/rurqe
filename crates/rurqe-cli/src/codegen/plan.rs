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

use rurqe_migrate::catalog::{ColumnDef, EnumDef, SchemaCatalog, TableDef};

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

/// Resolution of the Rust type of a generated column.
mod overrides;

use self::overrides::column_rust_type;

#[cfg(test)]
mod tests {
    use super::*;

    use rurqe_migrate::catalog::TypeKind;

    fn table(name: &str, columns: Vec<ColumnDef>) -> TableDef {
        TableDef::new(name, columns, Vec::new())
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
    fn a_secondary_schema_keeps_its_own_type_namespace() {
        // Both sections declare the very same table name, so the markers are
        // equal; only the section they are registered in keeps them apart.
        let catalog = SchemaCatalog::new(vec![
            table("users", vec![ColumnDef::new("id", TypeKind::Int64, false)]),
            table("users", vec![ColumnDef::new("id", TypeKind::Int64, false)]).with_schema("auth"),
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
