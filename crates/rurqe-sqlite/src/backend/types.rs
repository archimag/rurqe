//! Translation between typed SQL values and the `rusqlite` parameter interface.

use ::rusqlite::types::{ToSql, ToSqlOutput, ValueRef};

use rurqe_core::expr::SqlVal;

/// Bridges a [`SqlVal`] into the `rusqlite` parameter interface.
pub(super) struct SqlValParam<'a>(pub(super) &'a SqlVal);

impl ToSql for SqlValParam<'_> {
    fn to_sql(&self) -> ::rusqlite::Result<ToSqlOutput<'_>> {
        match self.0 {
            SqlVal::Null => Ok(ToSqlOutput::Borrowed(ValueRef::Null)),
            SqlVal::I32(value) => Ok(ToSqlOutput::Borrowed(ValueRef::Integer(i64::from(*value)))),
            SqlVal::I64(value) => Ok(ToSqlOutput::Borrowed(ValueRef::Integer(*value))),
            // SQLite has no storage class for NaN and converts a bound NaN
            // double to NULL, so a NaN parameter is refused instead of being
            // silently substituted with a value it never carried. Infinities
            // keep binding as-is: SQLite stores them faithfully as a REAL.
            SqlVal::F64(value) if value.is_nan() => Err(nan_failure()),
            SqlVal::F64(value) => Ok(ToSqlOutput::Borrowed(ValueRef::Real(*value))),
            SqlVal::Bool(value) => Ok(ToSqlOutput::Borrowed(ValueRef::Integer(*value as i64))),
            SqlVal::Text(value) => Ok(ToSqlOutput::Borrowed(ValueRef::Text(value.as_bytes()))),
            SqlVal::Bytes(value) => Ok(ToSqlOutput::Borrowed(ValueRef::Blob(value.as_slice()))),
            // SQLite has no array column type, so an array binds as the JSON
            // document of its elements: that is the form
            // `Vec<T>::from_sql_value` parses back into the elements of the
            // array, and the form an aggregated row set already arrives in.
            // Rendering the value through its `Display` form instead would
            // bind PostgreSQL's `{...}` spelling, which SQLite neither stores
            // as a document nor decodes back into a collection.
            SqlVal::Array(values) => Ok(ToSqlOutput::Owned(::rusqlite::types::Value::Text(
                json_array(values)?,
            ))),
            SqlVal::EncodingError(message) => Err(::rusqlite::Error::ToSqlConversionFailure(
                Box::new(::std::io::Error::other(message.clone())),
            )),
            // The variants left are the feature-gated scalar families
            // (`decimal`, `uuid`, `chrono`, `serde_json`), whose textual form
            // is the value SQLite stores for the column they belong to.
            #[allow(unreachable_patterns)]
            other => Ok(ToSqlOutput::Owned(::rusqlite::types::Value::Text(
                other.to_string(),
            ))),
        }
    }
}

/// Builds the conversion failure reported for a `NaN` double parameter.
///
/// SQLite has no storage class for `NaN` and stores a bound `NaN` as `NULL`,
/// so the parameter is refused instead of silently reaching the database as a
/// value it never carried. Infinities keep binding as-is, because SQLite
/// stores them faithfully.
fn nan_failure() -> ::rusqlite::Error {
    let cause =
        ::std::io::Error::other("a NaN double has no SQLite representation and would bind as NULL");
    ::rusqlite::Error::ToSqlConversionFailure(Box::new(cause))
}

/// Renders an array value as the JSON document bound for its parameter.
fn json_array(values: &[SqlVal]) -> ::rusqlite::Result<String> {
    let mut document = String::from("[");
    for (index, value) in values.iter().enumerate() {
        if index > 0 {
            document.push(',');
        }
        push_json_value(&mut document, value)?;
    }
    document.push(']');

    Ok(document)
}

/// Appends one element of an array document in its JSON form.
///
/// A value that cannot be encoded aborts the whole parameter instead of
/// entering the document as a substitute, so the driver reports the element
/// that failed rather than binding a document of the wrong shape.
fn push_json_value(out: &mut String, value: &SqlVal) -> ::rusqlite::Result<()> {
    match value {
        SqlVal::Null => out.push_str("null"),
        SqlVal::I32(value) => out.push_str(&value.to_string()),
        SqlVal::I64(value) => out.push_str(&value.to_string()),
        SqlVal::F64(value) if value.is_finite() => out.push_str(&value.to_string()),
        // JSON has no notation for a non-finite number, and the document of
        // an array parameter is the only place such a value would be written:
        // SQLite stores a top-level infinity faithfully as a REAL and refuses
        // a top-level NaN outright, so an element is refused here instead of
        // entering the document as `null`. The recursive call reports the same
        // refusal at every depth of a nested array.
        SqlVal::F64(number) => return Err(non_finite_array_failure(*number)),
        SqlVal::Bool(value) => out.push_str(if *value { "true" } else { "false" }),
        SqlVal::Text(value) => push_json_string(out, value),
        // A blob has no JSON counterpart, so its octets render as a list of
        // numbers, which keeps the document parseable.
        SqlVal::Bytes(octets) => {
            out.push('[');
            for (index, octet) in octets.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                out.push_str(&octet.to_string());
            }
            out.push(']');
        }
        SqlVal::Array(values) => {
            out.push('[');
            for (index, value) in values.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                push_json_value(out, value)?;
            }
            out.push(']');
        }
        SqlVal::EncodingError(message) => {
            return Err(::rusqlite::Error::ToSqlConversionFailure(Box::new(
                ::std::io::Error::other(message.clone()),
            )));
        }
        // A feature-gated scalar family renders in its textual form, which is
        // the value the crate parses back out of the element.
        #[allow(unreachable_patterns)]
        other => push_json_string(out, &other.to_string()),
    }

    Ok(())
}

/// Builds the conversion failure reported for a non-finite element of an array.
///
/// SQLite stores a top-level infinity faithfully as a REAL and refuses a
/// top-level `NaN` outright, so the scalar path keeps both behaviours. The JSON
/// document of an array parameter carries no notation for either, so an element
/// is refused instead of entering the document as `null`, a value the caller
/// never carried.
fn non_finite_array_failure(value: f64) -> ::rusqlite::Error {
    let cause = ::std::io::Error::other(format!(
        "a non-finite double ({value}) cannot be bound as an array parameter element: \
         its JSON document has no notation for it"
    ));
    ::rusqlite::Error::ToSqlConversionFailure(Box::new(cause))
}

/// Appends `value` as a quoted JSON string, escaping what JSON requires.
fn push_json_string(out: &mut String, value: &str) {
    out.push('"');
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            // JSON requires every other control character to be escaped.
            ch if (ch as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", ch as u32));
            }
            ch => out.push(ch),
        }
    }
    out.push('"');
}

/// Decodes a raw SQLite value into the matching [`SqlVal`] variant.
///
/// Text columns must hold valid UTF-8: a value that is not valid UTF-8 is
/// reported as a conversion failure for the offending column instead of being
/// silently rewritten with replacement characters.
pub(super) fn value_ref_to_sql_val(
    index: usize,
    value: ValueRef<'_>,
) -> Result<SqlVal, ::rusqlite::Error> {
    Ok(match value {
        ValueRef::Null => SqlVal::Null,
        ValueRef::Integer(value) => SqlVal::I64(value),
        ValueRef::Real(value) => SqlVal::F64(value),
        ValueRef::Text(bytes) => {
            SqlVal::Text(String::from_utf8(bytes.to_vec()).map_err(|error| {
                ::rusqlite::Error::FromSqlConversionFailure(
                    index,
                    ::rusqlite::types::Type::Text,
                    Box::new(error),
                )
            })?)
        }
        ValueRef::Blob(bytes) => SqlVal::Bytes(bytes.to_vec()),
    })
}
