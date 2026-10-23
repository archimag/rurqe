//! SQL array columns: the element contract and the `Vec<T>` value surface.
//!
//! An array column holds the ordered list of its elements, so the parameter of
//! such a column carries one [`SqlVal`] per element and row decoding reverses
//! that mapping in the same order. [`SqlArrayElement`] names the scalar types an
//! array is built from: implementing it is all it takes for `Vec<T>` to bind as a
//! query parameter, to decode from a result row, to lift into `Option<Vec<T>>`
//! when it is projected from an outer join, and to compare with a value inside an
//! expression.
//!
//! `u8` is deliberately not an element: a `Vec<u8>` column is the binary blob
//! column type, so reading it as an array of blobs would silently change the
//! meaning of every existing blob column.

use crate::expr::{ExprKind, IntoExpr, SqlVal, ToSqlVal};
use crate::query::MakeNullable;
use crate::row::{FromSqlValue, RowDecodeError};

/// A scalar type an SQL array column is built from.
///
/// The trait marks the two symmetric value contracts, so a type that binds as a
/// parameter and decodes from a result row is also usable as one element of an
/// array column. Implementations are provided for `i16`, `i32`, `u32`, `i64`,
/// `usize`, `f64`, `bool`, and `String`, for `Uuid`, the temporal types, and JSON
/// when their cargo features are enabled, and for every domain newtype deriving
/// `SqlType` or built with the declarative newtype macros.
///
/// ```
/// use rurqe_core::expr::{SqlVal, ToSqlVal};
///
/// let parameter = vec![1i64, 2i64].to_sql_val();
///
/// assert_eq!(parameter, SqlVal::Array(vec![SqlVal::I64(1), SqlVal::I64(2)]));
/// ```
#[diagnostic::on_unimplemented(
    message = "`{Self}` cannot be an array element",
    label = "unsupported array element type",
    note = "elements must implement `ToSqlVal` and `FromSqlValue`, which a `#[derive(SqlType)]` newtype does automatically"
)]
pub trait SqlArrayElement: ToSqlVal + FromSqlValue + Sized {}

/// Implements [`SqlArrayElement`] for the listed scalar types.
///
/// The list is written out instead of being derived from the value contracts so
/// that `u8` stays excluded: `Vec<u8>` is the binary blob column type, and a
/// blanket implementation would turn it into an array column.
macro_rules! impl_array_element {
    ($($element:ty),+ $(,)?) => {
        $(
            impl SqlArrayElement for $element {}
        )+
    };
}

impl_array_element!(i16, i32, u32, i64, usize, f64, bool, String);

#[cfg(feature = "uuid")]
impl_array_element!(uuid::Uuid);

#[cfg(feature = "chrono")]
impl_array_element!(
    chrono::NaiveDateTime,
    chrono::DateTime<chrono::Utc>,
    chrono::NaiveDate
);

#[cfg(feature = "serde_json")]
impl_array_element!(serde_json::Value);

impl<T: SqlArrayElement> ToSqlVal for Vec<T> {
    fn to_sql_val(self) -> SqlVal {
        SqlVal::Array(self.into_iter().map(ToSqlVal::to_sql_val).collect())
    }
}

impl<T: SqlArrayElement> FromSqlValue for Vec<T> {
    fn from_sql_value(value: &SqlVal) -> Result<Self, RowDecodeError> {
        match value {
            // An element that does not decode reports the element type, so the
            // failure names the column rather than the array wrapping it.
            SqlVal::Array(elements) => elements.iter().map(T::from_sql_value).collect(),
            // A nested collection arrives as JSON: PostgreSQL aggregates it into
            // a `jsonb` array while SQLite renders it as JSON text, and every item
            // of such an array is the JSON object of one row.
            #[cfg(feature = "serde_json")]
            SqlVal::Json(serde_json::Value::Array(items)) => items
                .iter()
                .map(|item| T::from_sql_value(&SqlVal::Json(item.clone())))
                .collect(),
            #[cfg(feature = "serde_json")]
            SqlVal::Text(text) => {
                Vec::<T>::from_sql_value(&SqlVal::Json(parse_json_document(text)?))
            }
            _ => Err(RowDecodeError::TypeMismatch {
                target: std::any::type_name::<Self>(),
                found: value.variant_name(),
            }),
        }
    }
}

/// Parses the JSON document held by a textual value.
#[cfg(feature = "serde_json")]
fn parse_json_document(text: &str) -> Result<serde_json::Value, RowDecodeError> {
    serde_json::from_str(text).map_err(|error| RowDecodeError::InvalidValue {
        target: "a JSON document",
        found: "text",
        detail: error.to_string(),
    })
}

impl<T: SqlArrayElement> MakeNullable for Vec<T> {
    type Nullable = Option<Vec<T>>;
}

impl<T: SqlArrayElement> IntoExpr<Vec<T>> for Vec<T> {
    type Scope = ();

    fn into_expr_kind(self) -> ExprKind {
        ExprKind::Value(self.to_sql_val())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_array_binds_one_value_per_element() {
        assert_eq!(
            vec![1i64, 2].to_sql_val(),
            SqlVal::Array(vec![SqlVal::I64(1), SqlVal::I64(2)])
        );
        assert_eq!(
            vec!["a".to_owned(), "b".to_owned()].to_sql_val(),
            SqlVal::Array(vec![
                SqlVal::Text("a".to_owned()),
                SqlVal::Text("b".to_owned()),
            ])
        );
        assert_eq!(
            vec![true, false].to_sql_val(),
            SqlVal::Array(vec![SqlVal::Bool(true), SqlVal::Bool(false)])
        );
    }

    #[test]
    fn an_array_round_trips_through_its_values() {
        let column = vec![1i64, 2, 3];
        let encoded = column.clone().to_sql_val();

        assert_eq!(Vec::<i64>::from_sql_value(&encoded), Ok(column));
        assert_eq!(
            Vec::<f64>::from_sql_value(&SqlVal::Array(vec![SqlVal::F64(0.5)])),
            Ok(vec![0.5])
        );
    }

    #[test]
    fn an_empty_array_binds_no_element() {
        assert_eq!(Vec::<i64>::new().to_sql_val(), SqlVal::Array(Vec::new()));
        assert_eq!(
            Vec::<i64>::from_sql_value(&SqlVal::Array(Vec::new())),
            Ok(Vec::new())
        );
    }

    #[test]
    fn a_scalar_value_is_rejected_for_an_array_column() {
        let error = Vec::<i64>::from_sql_value(&SqlVal::Null)
            .expect_err("a scalar does not decode into an array");

        assert!(matches!(
            error,
            RowDecodeError::TypeMismatch {
                found: "null",
                ..
            }
        ));
        assert!(error.to_string().contains("cannot decode a SQL `null` value"));
    }

    #[test]
    fn an_element_of_another_kind_is_reported() {
        let error = Vec::<i64>::from_sql_value(&SqlVal::Array(vec![SqlVal::Text("1".to_owned())]))
            .expect_err("a text element does not decode into an integer");

        assert!(matches!(
            error,
            RowDecodeError::TypeMismatch {
                found: "text",
                ..
            }
        ));
    }

    #[test]
    fn an_array_column_lifts_into_a_single_option() {
        fn assert_nullable<T: MakeNullable<Nullable = Option<T>>>() {}

        assert_nullable::<Vec<i64>>();
        assert_nullable::<Vec<String>>();
        assert_nullable::<Vec<bool>>();
    }

    #[test]
    fn an_optional_array_binds_null_and_decodes_it() {
        assert_eq!(None::<Vec<i64>>.to_sql_val(), SqlVal::Null);
        assert_eq!(
            Some(vec![1i64]).to_sql_val(),
            SqlVal::Array(vec![SqlVal::I64(1)])
        );
        assert_eq!(
            <Option<Vec<i64>> as FromSqlValue>::from_sql_value(&SqlVal::Null),
            Ok(None)
        );
    }

    #[test]
    fn an_array_operand_carries_its_elements_as_one_value() {
        assert_eq!(
            vec![1i64, 2].into_expr_kind(),
            ExprKind::Value(SqlVal::Array(vec![SqlVal::I64(1), SqlVal::I64(2)]))
        );
    }
}
