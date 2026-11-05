use super::*;
use crate::expr::ToSqlVal;
use crate::query::Selection;

crate::table!(
    User => "users" {
        UserId => "id": i64,
        UserName => "name": String,
        UserActive => "active": bool,
    }
);

#[test]
fn decodes_scalar_values() {
    assert_eq!(i16::from_sql_value(&SqlVal::I32(7)), Ok(7));
    assert_eq!(i32::from_sql_value(&SqlVal::I32(7)), Ok(7));
    assert_eq!(u32::from_sql_value(&SqlVal::I64(7)), Ok(7));
    assert_eq!(i64::from_sql_value(&SqlVal::I64(7)), Ok(7));
    assert_eq!(usize::from_sql_value(&SqlVal::I64(7)), Ok(7));
    assert_eq!(f64::from_sql_value(&SqlVal::F64(0.5)), Ok(0.5));
    assert_eq!(bool::from_sql_value(&SqlVal::Bool(true)), Ok(true));
    assert_eq!(
        String::from_sql_value(&SqlVal::Text("root".to_owned())),
        Ok("root".to_owned())
    );
    assert_eq!(
        Vec::<u8>::from_sql_value(&SqlVal::Bytes(vec![1, 2])),
        Ok(vec![1, 2])
    );
}

#[test]
fn rejects_values_of_other_variants() {
    assert_eq!(
        i64::from_sql_value(&SqlVal::Text("7".to_owned())),
        Err(RowDecodeError::TypeMismatch {
            target: std::any::type_name::<i64>(),
            found: "text",
        })
    );
    assert!(String::from_sql_value(&SqlVal::I64(7)).is_err());
    assert!(bool::from_sql_value(&SqlVal::Text("true".to_owned())).is_err());
    assert!(f64::from_sql_value(&SqlVal::Text("7".to_owned())).is_err());
}

#[cfg(feature = "uuid")]
#[test]
fn string_columns_decode_a_uuid_in_its_textual_form() {
    assert_eq!(
        String::from_sql_value(&SqlVal::Uuid(uuid::Uuid::from_u128(1))),
        Ok("00000000-0000-0000-0000-000000000001".to_owned())
    );
}

#[cfg(feature = "chrono")]
#[test]
fn string_columns_decode_temporal_values_in_their_textual_form() {
    let timestamp = chrono::NaiveDate::from_ymd_opt(2024, 1, 2)
        .and_then(|date| date.and_hms_opt(3, 4, 5))
        .expect("the timestamp is in the range of the calendar");

    assert_eq!(
        String::from_sql_value(&SqlVal::NaiveDateTime(timestamp)),
        Ok("2024-01-02 03:04:05".to_owned())
    );
    assert_eq!(
        String::from_sql_value(&SqlVal::NaiveDate(timestamp.date())),
        Ok("2024-01-02".to_owned())
    );
    assert_eq!(
        String::from_sql_value(&SqlVal::DateTimeUtc(
            chrono::DateTime::from_naive_utc_and_offset(timestamp, chrono::Utc),
        )),
        Ok("2024-01-02T03:04:05+00:00".to_owned())
    );
}

#[cfg(feature = "serde_json")]
#[test]
fn string_columns_decode_a_json_document_in_its_textual_form() {
    let document = serde_json::json!({ "active": true });

    assert_eq!(
        String::from_sql_value(&SqlVal::Json(document.clone())),
        Ok(document.to_string())
    );
}

#[cfg(feature = "decimal")]
#[test]
fn string_columns_decode_an_exact_decimal_in_its_textual_form() {
    assert_eq!(
        String::from_sql_value(&SqlVal::Decimal(rust_decimal::Decimal::new(1500, 3))),
        Ok("1.500".to_owned())
    );
}

#[test]
fn narrow_integers_accept_wide_integer_values() {
    assert_eq!(i32::from_sql_value(&SqlVal::I64(42)), Ok(42));
    assert_eq!(i16::from_sql_value(&SqlVal::I64(42)), Ok(42));
    assert_eq!(u32::from_sql_value(&SqlVal::I64(42)), Ok(42));
    assert_eq!(usize::from_sql_value(&SqlVal::I32(42)), Ok(42));
}

#[test]
fn out_of_range_values_report_invalid_representations() {
    let error = i32::from_sql_value(&SqlVal::I64(i64::MAX)).expect_err("overflow is rejected");
    assert!(matches!(
        error,
        RowDecodeError::InvalidValue { found: "int64", .. }
    ));
    assert!(error.to_string().contains("32-bit integer"));

    let error = u32::from_sql_value(&SqlVal::I32(-1)).expect_err("negatives are rejected");
    assert!(matches!(
        error,
        RowDecodeError::InvalidValue { found: "int32", .. }
    ));

    let error =
        i16::from_sql_value(&SqlVal::I64(i64::from(i32::MAX))).expect_err("overflow is rejected");
    assert!(matches!(
        error,
        RowDecodeError::InvalidValue { found: "int64", .. }
    ));
}

#[test]
fn booleans_accept_integer_encodings() {
    assert_eq!(bool::from_sql_value(&SqlVal::I64(1)), Ok(true));
    assert_eq!(bool::from_sql_value(&SqlVal::I32(0)), Ok(false));

    let error = bool::from_sql_value(&SqlVal::I64(2)).expect_err("only 0 and 1 decode");
    assert!(matches!(
        error,
        RowDecodeError::InvalidValue { found: "int64", .. }
    ));
    assert!(error.to_string().contains("0 or 1"));
}

#[test]
fn floating_point_values_accept_the_integral_kinds() {
    assert_eq!(f64::from_sql_value(&SqlVal::I32(7)), Ok(7.0));
    assert_eq!(f64::from_sql_value(&SqlVal::I64(7)), Ok(7.0));
    assert_eq!(f64::from_sql_value(&SqlVal::I64(-1200)), Ok(-1200.0));
    assert_eq!(f64::from_sql_value(&SqlVal::F64(0.5)), Ok(0.5));

    assert!(f64::from_sql_value(&SqlVal::Bool(true)).is_err());
    assert!(f64::from_sql_value(&SqlVal::Bytes(vec![1, 2])).is_err());
    assert!(f64::from_sql_value(&SqlVal::Null).is_err());
}

#[test]
fn integer_values_accept_a_floating_point_value_carrying_no_fraction() {
    assert_eq!(i64::from_sql_value(&SqlVal::F64(7.0)), Ok(7));
    assert_eq!(i64::from_sql_value(&SqlVal::F64(-1200.0)), Ok(-1200));

    let error = i64::from_sql_value(&SqlVal::F64(0.5)).expect_err("a fraction is rejected");
    assert!(matches!(
        error,
        RowDecodeError::InvalidValue {
            found: "float64",
            ..
        }
    ));

    assert!(i64::from_sql_value(&SqlVal::F64(f64::NAN)).is_err());
    assert!(i64::from_sql_value(&SqlVal::F64(f64::INFINITY)).is_err());
    assert!(i64::from_sql_value(&SqlVal::F64(1e30)).is_err());
}

#[test]
fn integer_values_report_the_edges_of_the_floating_point_range() {
    // `-2^63` is exactly `i64::MIN`, while `2^63` is outside the range and
    // previously decoded into `i64::MAX` through the saturating cast.
    assert_eq!(
        i64::from_sql_value(&SqlVal::F64(-9_223_372_036_854_775_808.0)),
        Ok(i64::MIN)
    );

    let error = i64::from_sql_value(&SqlVal::F64(9_223_372_036_854_775_808.0))
        .expect_err("2^63 lies outside the 64-bit integer range");
    assert!(matches!(
        error,
        RowDecodeError::InvalidValue {
            found: "float64",
            ..
        }
    ));
    assert!(error.to_string().contains("not an exact 64-bit integer"));
    assert!(i64::from_sql_value(&SqlVal::F64(f64::NEG_INFINITY)).is_err());

    // `2^53` is the largest integer a `f64` carries exactly: `2^53 + 1`
    // rounds to `2^53`, so that very number is the one read back instead of
    // a value the floating point representable set never held.
    let two_pow_53 = 9_007_199_254_740_992.0_f64;

    assert_eq!(
        i64::from_sql_value(&SqlVal::F64(two_pow_53)),
        Ok(9_007_199_254_740_992)
    );
    assert_eq!(
        i64::from_sql_value(&SqlVal::F64(two_pow_53 + 1.0)),
        Ok(9_007_199_254_740_992)
    );
}

#[test]
fn decodes_tuples_in_selection_order() {
    let row = <(i64, String, bool)>::try_from_row(&[
        SqlVal::I64(1),
        SqlVal::Text("root".to_owned()),
        SqlVal::Bool(true),
    ]);

    assert_eq!(row, Ok((1, "root".to_owned(), true)));
}

#[test]
fn single_element_tuple_is_supported() {
    assert_eq!(<(i64,)>::try_from_row(&[SqlVal::I64(1)]), Ok((1,)));
}

#[test]
fn arity_mismatch_is_reported_with_both_lengths() {
    assert_eq!(
        <(i64, String)>::try_from_row(&[SqlVal::I64(1)]),
        Err(RowDecodeError::ArityMismatch {
            expected: 2,
            actual: 1,
        })
    );
    assert_eq!(
        <(i64,)>::try_from_row(&[SqlVal::I64(1), SqlVal::I64(2)]),
        Err(RowDecodeError::ArityMismatch {
            expected: 1,
            actual: 2,
        })
    );
    assert!(RowDecodeError::ArityMismatch {
        expected: 2,
        actual: 1,
    }
    .to_string()
    .contains("row of 1 values"));
}

#[test]
fn value_type_mismatch_is_rejected() {
    assert_eq!(
        <(i64, String)>::try_from_row(&[
            SqlVal::Text("1".to_owned()),
            SqlVal::Text("root".to_owned()),
        ]),
        Err(RowDecodeError::TypeMismatch {
            target: std::any::type_name::<i64>(),
            found: "text",
        })
    );
}

#[test]
fn null_decodes_into_none_for_optional_values() {
    assert_eq!(<Option<i64>>::from_sql_value(&SqlVal::Null), Ok(None));
    assert_eq!(
        <Option<String>>::from_sql_value(&SqlVal::Text("root".to_owned())),
        Ok(Some("root".to_owned()))
    );
}

#[test]
fn optional_values_reject_foreign_variants() {
    assert!(<Option<i64>>::from_sql_value(&SqlVal::Text("7".to_owned())).is_err());
    assert!(<Option<i64>>::from_sql_value(&SqlVal::Null).is_ok());
}

#[test]
fn required_values_report_null_as_a_type_mismatch() {
    let error = i64::from_sql_value(&SqlVal::Null).expect_err("NULL needs an option");

    assert_eq!(
        error,
        RowDecodeError::TypeMismatch {
            target: std::any::type_name::<i64>(),
            found: "null",
        }
    );
}

#[test]
fn values_round_trip_through_the_symmetric_contracts() {
    fn round_trip<T>(value: T) -> T
    where
        T: ToSqlVal + FromSqlValue,
    {
        let encoded = value.to_sql_val();
        T::from_sql_value(&encoded).expect("an encoded value decodes again")
    }

    assert_eq!(round_trip(7i16), 7);
    assert_eq!(round_trip(7i32), 7);
    assert_eq!(round_trip(7u32), 7);
    assert_eq!(round_trip(7i64), 7);
    assert_eq!(round_trip(7usize), 7);
    assert_eq!(round_trip(0.5f64), 0.5);
    assert!(round_trip(true));
    assert_eq!(round_trip("root".to_owned()), "root");
    assert_eq!(round_trip(vec![1u8, 2]), vec![1, 2]);
    assert_eq!(round_trip(Some(7i64)), Some(7));
    assert_eq!(round_trip(None::<i64>), None);
}

#[test]
fn from_row_wraps_try_from_row() {
    assert_eq!(<(i64,)>::from_row(&[SqlVal::I64(1)]), Some((1,)));
    assert_eq!(<(i64,)>::from_row(&[]), None);
    assert_eq!(<(i64,)>::from_row(&[SqlVal::Text("1".to_owned())]), None);
}

#[derive(Debug, PartialEq, Eq)]
struct MockTxid([u8; 4]);

impl AsRef<[u8]> for MockTxid {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

impl TryFrom<&[u8]> for MockTxid {
    type Error = &'static str;

    fn try_from(slice: &[u8]) -> Result<Self, Self::Error> {
        if slice.len() == 4 {
            let mut arr = [0u8; 4];
            arr.copy_from_slice(slice);
            Ok(MockTxid(arr))
        } else {
            Err("invalid length")
        }
    }
}

crate::impl_sql_binary_newtype!(MockTxid, 4);

#[derive(Debug, PartialEq, Eq)]
struct MockAddress(String);

impl std::fmt::Display for MockAddress {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::str::FromStr for MockAddress {
    type Err = &'static str;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.starts_with("addr_") {
            Ok(MockAddress(s.to_owned()))
        } else {
            Err("missing addr_ prefix")
        }
    }
}

crate::impl_sql_text_newtype!(MockAddress);

#[test]
fn custom_binary_newtype_round_trips_and_validates_length() {
    let txid = MockTxid([1, 2, 3, 4]);
    let val = txid.to_sql_val();
    assert_eq!(val, SqlVal::Bytes(vec![1, 2, 3, 4]));
    assert_eq!(MockTxid::from_sql_value(&val), Ok(MockTxid([1, 2, 3, 4])));

    let wrong_len = SqlVal::Bytes(vec![1, 2, 3]);
    assert!(MockTxid::from_sql_value(&wrong_len).is_err());
}

#[test]
fn custom_text_newtype_round_trips_and_validates_parsing() {
    let addr = MockAddress("addr_12345".to_owned());
    let val = addr.to_sql_val();
    assert_eq!(val, SqlVal::Text("addr_12345".to_owned()));
    assert_eq!(
        MockAddress::from_sql_value(&val),
        Ok(MockAddress("addr_12345".to_owned()))
    );

    let invalid = SqlVal::Text("invalid_address".to_owned());
    assert!(MockAddress::from_sql_value(&invalid).is_err());
}

#[test]
fn binary_values_decode_from_their_hexadecimal_text() {
    assert_eq!(
        Vec::<u8>::from_sql_value(&SqlVal::Text("000102feff".to_owned())),
        Ok(vec![0x00, 0x01, 0x02, 0xfe, 0xff])
    );
    assert_eq!(
        Vec::<u8>::from_sql_value(&SqlVal::Text("DEAD".to_owned())),
        Ok(vec![0xde, 0xad]),
        "digits are case-insensitive"
    );
    assert_eq!(
        Vec::<u8>::from_sql_value(&SqlVal::Text(r"\x000102feff".to_owned())),
        Ok(vec![0x00, 0x01, 0x02, 0xfe, 0xff]),
        "the bytea text PostgreSQL reports decodes"
    );
    assert_eq!(
        Vec::<u8>::from_sql_value(&SqlVal::Bytes(vec![1, 2])),
        Ok(vec![1, 2]),
        "a binary value keeps its native kind"
    );
}

#[test]
fn binary_values_report_text_that_is_not_hexadecimal() {
    let error = Vec::<u8>::from_sql_value(&SqlVal::Text("[1, 2]".to_owned()))
        .expect_err("the debug rendering of a vector is not hexadecimal");

    assert!(matches!(
        error,
        RowDecodeError::InvalidValue { found: "text", .. }
    ));
    assert!(error.to_string().contains("hexadecimal"));
    assert!(Vec::<u8>::from_sql_value(&SqlVal::Text("010".to_owned())).is_err());
    assert!(Vec::<u8>::from_sql_value(&SqlVal::Text("zz".to_owned())).is_err());
    assert!(Vec::<u8>::from_sql_value(&SqlVal::Text("hello".to_owned())).is_err());
    assert!(Vec::<u8>::from_sql_value(&SqlVal::Bool(true)).is_err());
}

#[test]
fn hexadecimal_text_decodes_into_its_bytes() {
    assert_eq!(hex_decode("0001feff"), Some(vec![0x00, 0x01, 0xfe, 0xff]));
    assert_eq!(hex_decode("0001FEFF"), Some(vec![0x00, 0x01, 0xfe, 0xff]));
    assert_eq!(
        hex_decode(r"\x0001feff"),
        Some(vec![0x00, 0x01, 0xfe, 0xff])
    );
    assert_eq!(hex_decode(r"\x"), Some(Vec::new()));
    assert_eq!(hex_decode("010"), None);
    assert_eq!(hex_decode("zz"), None);
    assert_eq!(hex_decode("hello"), None);
}

#[test]
fn selection_rows_are_decodable() {
    fn assert_from_row<R: FromRow>() {}

    assert_from_row::<<(UserId, UserName, UserActive) as Selection>::Row>();
}

#[cfg(feature = "decimal")]
#[test]
fn floating_point_values_accept_an_exact_decimal() {
    assert_eq!(
        f64::from_sql_value(&SqlVal::Decimal(rust_decimal::Decimal::new(125, 1))),
        Ok(12.5)
    );
}

#[cfg(feature = "decimal")]
#[test]
fn integer_values_accept_an_exact_decimal() {
    use rust_decimal::Decimal;

    assert_eq!(
        i64::from_sql_value(&SqlVal::Decimal(Decimal::from(7i64))),
        Ok(7)
    );

    let error = i64::from_sql_value(&SqlVal::Decimal(Decimal::MAX))
        .expect_err("the value exceeds the 64-bit integer range");
    assert!(matches!(
        error,
        RowDecodeError::InvalidValue {
            found: "decimal",
            ..
        }
    ));
}

#[cfg(feature = "decimal")]
#[test]
fn decimal_values_decode_exactly() {
    use rust_decimal::Decimal;

    let exact = Decimal::new(1500, 3);

    assert_eq!(Decimal::from_sql_value(&SqlVal::Decimal(exact)), Ok(exact));
    assert_eq!(
        Decimal::from_sql_value(&SqlVal::I64(7)),
        Ok(Decimal::from(7i64))
    );
    assert_eq!(
        Decimal::from_sql_value(&SqlVal::Text("1.500".to_owned())),
        Ok(exact)
    );
    assert_eq!(
        Decimal::from_sql_value(&SqlVal::F64(1.5)),
        Ok(Decimal::new(15, 1))
    );
    assert!(Decimal::from_sql_value(&SqlVal::Bool(true)).is_err());
}
