//! Compatibility between a bound value kind and the column receiving it.
//!
//! The column whitelist of the binding layer decides whether a parameter may be
//! bound at all; it does not decide whether a value belongs in that column. A
//! value of an unrelated kind — binary octets in a text column, a boolean in a
//! binary one — would otherwise reach the wire and be reported by the server as
//! an encoding or protocol failure. The predicate below names the column families
//! each value kind is encoded into, so a mismatch fails with the diagnostic the
//! adapter itself produces and no wire bytes are written.

use rurqe_core::expr::SqlVal;

use super::driver::types::{Kind, Type};

/// Reports whether a value of the given kind can be encoded into a column of type
/// `column_ty`.
///
/// The column type is the one its domain chain resolves to, so a domain accepts
/// exactly the values of the type it is declared over. `NULL` belongs in every
/// column, and an encoding failure is reported by its own message rather than as a
/// kind mismatch.
pub(super) fn value_fits(column_ty: &Type, value: &SqlVal) -> bool {
    let integer = is_integer_column(column_ty);
    let float = is_float_column(column_ty);
    let text = is_text_column(column_ty);

    match value {
        SqlVal::Null => true,
        SqlVal::Bool(_) => column_ty == &Type::BOOL,
        SqlVal::I32(_) | SqlVal::I64(_) => integer,
        SqlVal::F64(_) => float,
        #[cfg(feature = "decimal")]
        SqlVal::Decimal(_) => integer || float || text,
        SqlVal::Text(_) => text || is_textual_target(column_ty),
        SqlVal::Bytes(_) => column_ty == &Type::BYTEA,
        // A typed wire value declares the SQL type whose octets it carries, so
        // it fits exactly the column declared over that type. The binding gate
        // compares the same names before the encoder is reached.
        SqlVal::TypedWire { sql_type, .. } => sql_type.as_str() == column_ty.name(),
        #[cfg(feature = "uuid")]
        SqlVal::Uuid(_) => column_ty == &Type::UUID || text,
        #[cfg(feature = "chrono")]
        SqlVal::NaiveDateTime(_) | SqlVal::DateTimeUtc(_) => {
            column_ty == &Type::TIMESTAMP || column_ty == &Type::TIMESTAMPTZ || text
        }
        #[cfg(feature = "chrono")]
        SqlVal::NaiveDate(_) => column_ty == &Type::DATE || text,
        #[cfg(feature = "serde_json")]
        SqlVal::Json(_) => column_ty == &Type::JSON || column_ty == &Type::JSONB || text,
        // An array value is encoded by the array codec before this point, and an
        // encoding failure carries the message of its own arm.
        SqlVal::Array(_) => false,
        SqlVal::EncodingError(_) => true,
        // `SqlVal` is `#[non_exhaustive]`, so a variant this build does not know is
        // refused instead of being encoded on a guess.
        _ => false,
    }
}

/// Whether `ty` is one of the integer column types.
fn is_integer_column(ty: &Type) -> bool {
    ty == &Type::INT2 || ty == &Type::INT4 || ty == &Type::INT8
}

/// Whether `ty` is one of the floating point column types.
fn is_float_column(ty: &Type) -> bool {
    ty == &Type::FLOAT4 || ty == &Type::FLOAT8
}

/// Whether `ty` carries UTF-8 text: the character types, `citext`, and enum labels.
fn is_text_column(ty: &Type) -> bool {
    let character_type = matches!(
        ty,
        &Type::TEXT | &Type::VARCHAR | &Type::BPCHAR | &Type::NAME | &Type::UNKNOWN
    );
    character_type || ty.name() == "citext" || matches!(ty.kind(), Kind::Enum(_))
}

/// Whether `ty` is a column that receives the textual form of a value and parses
/// it back into the type the column declares.
///
/// The UUID, JSON, and temporal types have no dedicated [`SqlVal`] variant in the
/// schema the code generator produces, which maps them onto `String`, so such a
/// column is bound with the text of the value and the adapter parses it before
/// encoding.
fn is_textual_target(ty: &Type) -> bool {
    matches!(
        ty,
        &Type::UUID
            | &Type::JSON
            | &Type::JSONB
            | &Type::TIMESTAMP
            | &Type::TIMESTAMPTZ
            | &Type::DATE
    )
}
