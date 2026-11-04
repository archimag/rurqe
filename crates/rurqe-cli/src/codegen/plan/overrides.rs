//! Resolution of the Rust type generated for a column.
//!
//! The resolution is driven by the neutral type the catalog reports and by the
//! project configuration: an explicit `table.column` override named by the
//! configuration wins over the domain mapping of the catalog type, which wins
//! over the default mapping whose names need no dependency beyond the library
//! itself.

use rurqe_migrate::catalog::{ColumnDef, TypeKind};

use crate::codegen::naming::table_type_ident;
use crate::codegen::CodegenOptions;

/// Resolves the Rust type generated for `column`, honouring project overrides.
///
/// A column of the primary schema is overridden through its `table.column`
/// path, while a column of a secondary schema is overridden through the
/// `schema.table.column` path the configuration file produces for it.
///
/// An override replaces the mapped type entirely and keeps the column's
/// nullability, so `uuid::Uuid` stays `uuid::Uuid` for a `NOT NULL` column and
/// becomes `Option<uuid::Uuid>` for a nullable one.
pub(super) fn column_rust_type(
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

    fn rust_type_of(column: &ColumnDef, options: &CodegenOptions) -> String {
        column_rust_type(None, "users", column, options)
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
}
