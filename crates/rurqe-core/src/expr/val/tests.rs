use super::*;
use crate::row::{FromSqlValue, RowDecodeError};

#[test]
fn value_kinds_report_stable_names() {
    assert_eq!(SqlVal::Null.variant_name(), "null");
    assert_eq!(SqlVal::I32(1).variant_name(), "int32");
    assert_eq!(SqlVal::I64(1).variant_name(), "int64");
    assert_eq!(SqlVal::F64(1.0).variant_name(), "float64");
    assert_eq!(SqlVal::Bool(true).variant_name(), "boolean");
    assert_eq!(SqlVal::Text(String::new()).variant_name(), "text");
    assert_eq!(SqlVal::Bytes(Vec::new()).variant_name(), "bytes");
    #[cfg(feature = "uuid")]
    assert_eq!(SqlVal::Uuid(uuid::Uuid::nil()).variant_name(), "uuid");
    #[cfg(feature = "chrono")]
    {
        assert_eq!(
            SqlVal::NaiveDateTime(chrono::NaiveDateTime::default()).variant_name(),
            "timestamp"
        );
        assert_eq!(
            SqlVal::DateTimeUtc(chrono::DateTime::default()).variant_name(),
            "timestamptz"
        );
        assert_eq!(
            SqlVal::NaiveDate(chrono::NaiveDate::default()).variant_name(),
            "date"
        );
    }
    #[cfg(feature = "serde_json")]
    assert_eq!(SqlVal::Json(serde_json::Value::Null).variant_name(), "json");
}

#[test]
fn to_sql_val_encodes_scalar_values() {
    assert_eq!(7i16.to_sql_val(), SqlVal::I32(7));
    assert_eq!(7i32.to_sql_val(), SqlVal::I32(7));
    assert_eq!(7u32.to_sql_val(), SqlVal::I64(7));
    assert_eq!(7i64.to_sql_val(), SqlVal::I64(7));
    assert_eq!(7usize.to_sql_val(), SqlVal::I64(7));
    assert_eq!(0.5f64.to_sql_val(), SqlVal::F64(0.5));
    assert_eq!(true.to_sql_val(), SqlVal::Bool(true));
    assert_eq!(
        String::from("root").to_sql_val(),
        SqlVal::Text("root".to_owned())
    );
    assert_eq!(Vec::from([1u8, 2]).to_sql_val(), SqlVal::Bytes(vec![1, 2]));
    assert_eq!(SqlVal::I64(1).to_sql_val(), SqlVal::I64(1));
}

#[test]
fn borrowed_values_are_encoded_by_copy() {
    let name = String::from("root");
    let bytes: &[u8] = &[1, 2];

    assert_eq!("root".to_sql_val(), SqlVal::Text("root".to_owned()));
    assert_eq!((&name).to_sql_val(), SqlVal::Text("root".to_owned()));
    assert_eq!(bytes.to_sql_val(), SqlVal::Bytes(vec![1, 2]));
    assert_eq!(name, "root");
}

#[test]
fn optional_values_encode_none_as_null() {
    assert_eq!(Some(7i64).to_sql_val(), SqlVal::I64(7));
    assert_eq!(None::<i64>.to_sql_val(), SqlVal::Null);
    assert_eq!(
        Some(String::from("root")).to_sql_val(),
        SqlVal::Text("root".to_owned())
    );
}

#[test]
fn optional_values_convert_into_sql_values() {
    assert_eq!(SqlVal::from(Some(7i64)), SqlVal::I64(7));
    assert_eq!(SqlVal::from(None::<i64>), SqlVal::Null);
    assert_eq!(
        SqlVal::from(Some(String::from("root"))),
        SqlVal::Text("root".to_owned())
    );
    assert_eq!(SqlVal::from(None::<String>), SqlVal::Null);
    assert_eq!(SqlVal::from(Some("root")), SqlVal::Text("root".to_owned()));
    let bytes: &[u8] = &[1, 2];
    assert_eq!(SqlVal::from(Some(bytes)), SqlVal::Bytes(vec![1, 2]));
}

#[cfg(feature = "serde_json")]
#[test]
fn optional_json_values_convert_into_sql_values() {
    assert_eq!(
        SqlVal::from(Some(Json(1i64))),
        SqlVal::Json(serde_json::json!(1))
    );
    assert_eq!(SqlVal::from(None::<Json<i64>>), SqlVal::Null);
}

#[cfg(feature = "decimal")]
#[test]
fn an_optional_borrowed_decimal_converts_into_its_sql_value() {
    let decimal = rust_decimal::Decimal::new(15_005, 3);

    assert_eq!(SqlVal::from(Some(&decimal)), SqlVal::Decimal(decimal));
    assert_eq!(SqlVal::from(None::<&rust_decimal::Decimal>), SqlVal::Null);
}

#[test]
fn from_conversions_delegate_to_the_trait() {
    assert_eq!(SqlVal::from(7i64), 7i64.to_sql_val());
    assert_eq!(SqlVal::from(7u32), 7u32.to_sql_val());
    assert_eq!(SqlVal::from("root"), "root".to_sql_val());
    assert_eq!(SqlVal::from(1.5f64), 1.5f64.to_sql_val());
}

#[test]
fn encoding_failures_carry_their_reason() {
    let failure = SqlVal::EncodingError("cannot encode".to_owned());

    assert_eq!(failure.variant_name(), "encoding error");
    assert_eq!(failure.to_string(), "cannot encode");
}

#[test]
fn indices_within_the_signed_range_encode_as_integers() {
    assert_eq!(7usize.to_sql_val(), SqlVal::I64(7));
    assert_eq!(0usize.to_sql_val(), SqlVal::I64(0));
}

#[cfg(target_pointer_width = "64")]
#[test]
fn indices_above_the_signed_range_report_a_failure() {
    let wide = (i64::MAX as usize) + 1;

    let encoded = wide.to_sql_val();
    assert!(matches!(encoded, SqlVal::EncodingError(_)));
    assert!(encoded.to_string().contains("64-bit signed integer"));
}

#[cfg(feature = "serde_json")]
#[test]
fn unencodable_json_values_report_the_failure() {
    struct Unencodable;

    impl serde::Serialize for Unencodable {
        fn serialize<S: serde::Serializer>(&self, _: S) -> Result<S::Ok, S::Error> {
            Err(serde::ser::Error::custom("not serialisable"))
        }
    }

    let encoded = Json(Unencodable).to_sql_val();
    assert!(matches!(encoded, SqlVal::EncodingError(_)));
    assert!(encoded.to_string().contains("not serialisable"));

    assert_eq!(Json(1i64).to_sql_val(), SqlVal::Json(serde_json::json!(1)));
}

#[cfg(feature = "decimal")]
#[test]
fn exact_decimals_encode_as_their_own_kind() {
    let decimal = rust_decimal::Decimal::new(15_005, 3);

    assert_eq!(decimal.to_sql_val(), SqlVal::Decimal(decimal));
    assert_eq!((&decimal).to_sql_val(), SqlVal::Decimal(decimal));
    assert_eq!(SqlVal::from(decimal), SqlVal::Decimal(decimal));
    assert_eq!(Some(decimal).to_sql_val(), SqlVal::Decimal(decimal));
    assert_eq!(None::<rust_decimal::Decimal>.to_sql_val(), SqlVal::Null);
}

#[cfg(feature = "decimal")]
#[test]
fn the_decimal_kind_reports_its_name_and_text() {
    let value = SqlVal::Decimal(rust_decimal::Decimal::new(1500, 3));

    assert_eq!(value.variant_name(), "decimal");
    assert_eq!(value.to_string(), "1.500");
}

macro_rules! four_octets {
    ($name:ident) => {
        #[derive(Debug, PartialEq, Eq)]
        struct $name([u8; 4]);

        impl AsRef<[u8]> for $name {
            fn as_ref(&self) -> &[u8] {
                &self.0
            }
        }

        impl TryFrom<&[u8]> for $name {
            type Error = &'static str;

            fn try_from(bytes: &[u8]) -> Result<Self, Self::Error> {
                bytes
                    .try_into()
                    .map($name)
                    .map_err(|_| "expected four octets")
            }
        }
    };
}

four_octets!(Span);
four_octets!(Plain);

#[derive(Debug, PartialEq, Eq)]
struct Loose(Vec<u8>);

impl AsRef<[u8]> for Loose {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

impl TryFrom<&[u8]> for Loose {
    type Error = &'static str;

    fn try_from(bytes: &[u8]) -> Result<Self, Self::Error> {
        if bytes.is_empty() {
            Err("expected at least one octet")
        } else {
            Ok(Loose(bytes.to_vec()))
        }
    }
}

crate::impl_sql_binary_newtype!(Span, 4, sql_type = "span");
crate::impl_sql_binary_newtype!(Plain, 4);
crate::impl_sql_binary_newtype!(Loose, sql_type = "loose");

#[test]
fn the_typed_wire_kind_reports_its_name_and_octets() {
    let value = SqlVal::TypedWire {
        sql_type: "interval".to_owned(),
        payload: vec![0x0a, 0xff],
    };
    let empty = SqlVal::TypedWire {
        sql_type: "interval".to_owned(),
        payload: Vec::new(),
    };

    assert_eq!(value.variant_name(), "typed wire");
    assert_eq!(value.to_string(), "interval(0x0aff)");
    assert_eq!(empty.to_string(), "interval(0x)");
}

#[test]
fn a_typed_binary_newtype_encodes_as_a_typed_wire_value() {
    let expected = SqlVal::TypedWire {
        sql_type: "span".to_owned(),
        payload: vec![1, 2, 3, 4],
    };
    let owned = Span([1, 2, 3, 4]);

    assert_eq!(ToSqlVal::to_sql_val(&owned), expected);
    assert_eq!(SqlVal::from(owned), expected);
}

#[test]
fn a_typed_binary_newtype_decodes_typed_wire_values_and_plain_bytes() {
    let typed = SqlVal::TypedWire {
        sql_type: "span".to_owned(),
        payload: vec![1, 2, 3, 4],
    };

    assert_eq!(Span::from_sql_value(&typed), Ok(Span([1, 2, 3, 4])));
    assert_eq!(
        Span::from_sql_value(&SqlVal::Bytes(vec![1, 2, 3, 4])),
        Ok(Span([1, 2, 3, 4]))
    );
}

#[test]
fn a_typed_binary_newtype_checks_the_width_on_both_paths() {
    let narrow = SqlVal::Bytes(vec![1, 2, 3]);
    let wide = SqlVal::TypedWire {
        sql_type: "span".to_owned(),
        payload: vec![1, 2, 3, 4, 5],
    };

    for value in [narrow, wide] {
        assert!(matches!(
            Span::from_sql_value(&value),
            Err(RowDecodeError::InvalidValue { .. })
        ));
    }
}

#[test]
fn a_typed_binary_newtype_rejects_other_kinds_of_value() {
    let result = Span::from_sql_value(&SqlVal::Text("abcd".to_owned()));

    assert!(matches!(
        result,
        Err(RowDecodeError::TypeMismatch { found: "text", .. })
    ));
}

#[test]
fn a_typed_binary_newtype_without_a_width_accepts_any_length() {
    let typed = SqlVal::TypedWire {
        sql_type: "loose".to_owned(),
        payload: vec![9; 7],
    };

    assert_eq!(Loose::from_sql_value(&typed), Ok(Loose(vec![9; 7])));
    assert_eq!(
        Loose(vec![9, 9]).to_sql_val(),
        SqlVal::TypedWire {
            sql_type: "loose".to_owned(),
            payload: vec![9, 9],
        }
    );
}

#[test]
fn an_untyped_binary_newtype_keeps_the_plain_blob_contract() {
    let plain = Plain([1, 2, 3, 4]);
    let typed = SqlVal::TypedWire {
        sql_type: "span".to_owned(),
        payload: vec![1, 2, 3, 4],
    };

    assert_eq!(plain.to_sql_val(), SqlVal::Bytes(vec![1, 2, 3, 4]));
    assert_eq!(
        Plain::from_sql_value(&SqlVal::Bytes(vec![1, 2, 3, 4])),
        Ok(Plain([1, 2, 3, 4]))
    );
    assert!(matches!(
        Plain::from_sql_value(&typed),
        Err(RowDecodeError::TypeMismatch {
            found: "typed wire",
            ..
        })
    ));
}
