//! Mapping of the SQL types an engine declares onto the engine-neutral kind.
//!
//! The mapping serves the two report styles the workspace meets: PostgreSQL
//! reports parameter-free type names through `information_schema`, while SQLite
//! keeps the declared type verbatim and classifies it through its affinity rules.

use crate::catalog::TypeKind;

/// Maps a raw SQL database data type string to an engine-neutral [`TypeKind`].
///
/// The mapping serves engines that report parameter-free type names, as
/// PostgreSQL does through `information_schema.columns.data_type`, and stays
/// tolerant of a parameterised spelling such as `varchar(255)`. A type outside
/// the built-in set keeps its declared spelling, type parameters included, so an
/// `interval(6)` column never compares equal to an `interval(3)` one inside
/// `schema.lock`.
///
/// An array spelling such as `text[]` maps to [`TypeKind::Array`] of the type its
/// elements are declared as.
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
        "numeric" | "decimal" => TypeKind::Numeric,
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
/// affinity* rather than a fixed type system, so `VARCHAR(255)`, `UNSIGNED BIG
/// INT`, and `BOOLEAN` all have to be recognised from their spelling. The affinity
/// rules of the SQLite documentation are applied in order:
///
/// 1. a declared type containing `INT` has an integer affinity, and SQLite stores
///    every integer column as a 64-bit value, so it maps to [`TypeKind::Int64`];
/// 2. a type containing `CHAR`, `CLOB`, or `TEXT` maps to [`TypeKind::Text`];
/// 3. a type containing `BLOB`, or an absent declared type, maps to
///    [`TypeKind::Bytes`];
/// 4. a type containing `REAL`, `FLOA`, or `DOUB` maps to [`TypeKind::Float64`];
/// 5. a type containing `NUMERIC` or `DECIMAL` maps to [`TypeKind::Numeric`].
///
/// Declared types naming a domain rather than a storage class (`BOOLEAN`, `UUID`,
/// `JSON`, `DATE`, `DATETIME`) are recognised next, because SQLite stores their
/// values as the text or integer the matching Rust type decodes. A remaining
/// declared type has the numeric affinity of SQLite, which keeps a value either as
/// an integer or as a real depending on its magnitude and precision, so no single
/// Rust type reads such a column reliably; the declared spelling is preserved
/// through [`TypeKind::Other`] instead of guessing.
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
    } else if lower.contains("numeric") || lower.contains("decimal") {
        TypeKind::Numeric
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
