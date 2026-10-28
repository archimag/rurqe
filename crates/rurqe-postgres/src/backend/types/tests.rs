//! Regression tests for the binding layer.
//!
//! The tests drive the codecs through the type descriptors a server would report
//! for a column, so the wire representation of a column the code generator maps
//! onto `String` stays pinned without a running database.

use bytes::BytesMut;
use rurqe_core::expr::SqlVal;

use super::driver::types::{IsNull, Kind, ToSql, Type};
use super::wire::{wire_of, Wire};
use super::{decode_value, SqlValParam};

/// Builds a type the way the server would report it.
fn synthetic(name: &str, oid: u32, kind: Kind) -> Type {
    Type::new(name.to_owned(), oid, kind, "public".to_owned())
}

/// Builds the array type over `element`, the way the server would report it.
fn array_of(name: &str, oid: u32, element: Type) -> Type {
    Type::new(
        name.to_owned(),
        oid,
        Kind::Array(element),
        "public".to_owned(),
    )
}

#[test]
fn scalar_values_map_to_driver_parameters() {
    let mut buffer = BytesMut::new();
    assert!(matches!(
        SqlValParam(&SqlVal::I64(7))
            .to_sql(&Type::INT8, &mut buffer)
            .unwrap(),
        IsNull::No
    ));
    assert!(matches!(
        SqlValParam(&SqlVal::Bool(true))
            .to_sql(&Type::BOOL, &mut buffer)
            .unwrap(),
        IsNull::No
    ));
    assert!(matches!(
        SqlValParam(&SqlVal::Text("root".to_owned()))
            .to_sql(&Type::TEXT, &mut buffer)
            .unwrap(),
        IsNull::No
    ));
}

#[test]
fn a_smallint_column_binds_two_octets() {
    let mut buffer = BytesMut::new();
    assert!(matches!(
        SqlValParam(&SqlVal::I32(7))
            .to_sql_checked(&Type::INT2, &mut buffer)
            .unwrap(),
        IsNull::No
    ));

    // A `smallint` column reads two octets, so the parameter must not carry the
    // four octets of the 32 bit value it was bound from.
    assert_eq!(buffer.to_vec(), vec![0u8, 7]);
}

#[test]
fn a_real_column_binds_four_octets() {
    let mut buffer = BytesMut::new();
    assert!(matches!(
        SqlValParam(&SqlVal::F64(0.5))
            .to_sql_checked(&Type::FLOAT4, &mut buffer)
            .unwrap(),
        IsNull::No
    ));

    assert_eq!(buffer.to_vec(), 0.5f32.to_be_bytes().to_vec());
}

#[test]
fn an_integer_binds_at_the_width_of_its_column() {
    let mut buffer = BytesMut::new();
    SqlValParam(&SqlVal::I64(9))
        .to_sql_checked(&Type::INT4, &mut buffer)
        .unwrap();
    assert_eq!(buffer.to_vec(), 9i32.to_be_bytes().to_vec());

    let mut buffer = BytesMut::new();
    SqlValParam(&SqlVal::I32(9))
        .to_sql_checked(&Type::INT8, &mut buffer)
        .unwrap();
    assert_eq!(buffer.to_vec(), 9i64.to_be_bytes().to_vec());
}

#[test]
fn a_value_out_of_the_range_of_a_smallint_column_is_rejected() {
    let mut buffer = BytesMut::new();
    let error = match SqlValParam(&SqlVal::I32(i32::from(i16::MAX) + 1))
        .to_sql_checked(&Type::INT2, &mut buffer)
    {
        Ok(_) => panic!("the value does not fit a smallint column"),
        Err(error) => error,
    };

    assert!(error.to_string().contains("out of the range"));
    assert!(error.to_string().contains("int2"));
}

#[test]
fn a_value_out_of_the_range_of_a_real_column_is_rejected() {
    let mut buffer = BytesMut::new();
    let error = match SqlValParam(&SqlVal::F64(f64::MAX)).to_sql_checked(&Type::FLOAT4, &mut buffer)
    {
        Ok(_) => panic!("the value does not fit a real column"),
        Err(error) => error,
    };

    assert!(error.to_string().contains("out of the range"));
    assert!(error.to_string().contains("float4"));
}

#[test]
fn a_domain_binds_the_representation_of_the_type_it_is_declared_over() {
    let smallint_domain = synthetic("small_count", 65_527, Kind::Domain(Type::INT2));
    let mut buffer = BytesMut::new();
    assert!(matches!(
        SqlValParam(&SqlVal::I32(3))
            .to_sql_checked(&smallint_domain, &mut buffer)
            .unwrap(),
        IsNull::No
    ));
    assert_eq!(buffer.to_vec(), vec![0u8, 3]);

    let decimal_domain = synthetic("rated", 65_526, Kind::Domain(Type::NUMERIC));
    let mut buffer = BytesMut::new();
    SqlValParam(&SqlVal::Text("12.5".to_owned()))
        .to_sql_checked(&decimal_domain, &mut buffer)
        .unwrap();
    assert_eq!(Wire::Numeric.decode(&buffer[..]), Ok("12.5".to_owned()));
}

#[cfg(feature = "uuid")]
#[test]
fn a_domain_over_uuid_binds_its_textual_value() {
    let uuid_domain = synthetic("user_id", 65_525, Kind::Domain(Type::UUID));
    let mut buffer = BytesMut::new();
    assert!(matches!(
        SqlValParam(&SqlVal::Text(
            "00000000-0000-0000-0000-000000000000".to_owned()
        ))
        .to_sql_checked(&uuid_domain, &mut buffer)
        .unwrap(),
        IsNull::No
    ));

    assert_eq!(buffer.to_vec(), vec![0u8; 16]);
}

#[test]
fn null_binds_for_any_declared_type() {
    let mut buffer = BytesMut::new();
    assert!(matches!(
        SqlValParam(&SqlVal::Null)
            .to_sql(&Type::INT8, &mut buffer)
            .unwrap(),
        IsNull::Yes
    ));
    assert!(matches!(
        SqlValParam(&SqlVal::Null)
            .to_sql(&Type::UUID, &mut buffer)
            .unwrap(),
        IsNull::Yes
    ));
}

#[test]
fn unusable_column_types_are_rejected_with_a_clear_message() {
    let mut buffer = BytesMut::new();
    let error = match SqlValParam(&SqlVal::I64(1)).to_sql_checked(&Type::INTERVAL, &mut buffer) {
        Ok(_) => panic!("intervals cannot receive bound parameters"),
        Err(error) => error,
    };

    assert!(error.to_string().contains("int64"));
    assert!(error.to_string().contains("interval"));
}

#[test]
fn null_binds_to_columns_outside_the_supported_types() {
    let mut buffer = BytesMut::new();

    assert!(matches!(
        SqlValParam(&SqlVal::Null)
            .to_sql_checked(&Type::INTERVAL, &mut buffer)
            .unwrap(),
        IsNull::Yes
    ));
}

#[test]
fn encoding_failures_surface_as_driver_errors() {
    let mut buffer = BytesMut::new();
    let error = match SqlValParam(&SqlVal::EncodingError("not encodable".to_owned()))
        .to_sql_checked(&Type::TEXT, &mut buffer)
    {
        Ok(_) => panic!("an encoding failure cannot be bound"),
        Err(error) => error,
    };

    assert!(error.to_string().contains("not encodable"));
}

#[test]
fn numeric_columns_bind_the_decimal_text_of_the_value() {
    let mut buffer = BytesMut::new();
    let param = SqlValParam(&SqlVal::Text("12345.678".to_owned()));
    assert!(matches!(
        param.to_sql_checked(&Type::NUMERIC, &mut buffer).unwrap(),
        IsNull::No
    ));
    assert_eq!(
        buffer.to_vec(),
        vec![0u8, 3, 0, 1, 0, 0, 0, 3, 0, 1, 9, 0x29, 0x1a, 0x7c]
    );

    // A value bound as a number reaches the same decimal representation.
    let mut buffer = BytesMut::new();
    SqlValParam(&SqlVal::I64(42))
        .to_sql_checked(&Type::NUMERIC, &mut buffer)
        .unwrap();
    assert_eq!(buffer.to_vec(), vec![0u8, 1, 0, 0, 0, 0, 0, 0, 0, 42]);
    assert_eq!(Wire::Numeric.decode(&buffer[..]), Ok("42".to_owned()));
}

#[test]
fn network_columns_bind_and_decode_their_textual_address() {
    let mut buffer = BytesMut::new();
    SqlValParam(&SqlVal::Text("10.1.2.0/24".to_owned()))
        .to_sql_checked(&Type::CIDR, &mut buffer)
        .unwrap();
    assert_eq!(buffer.to_vec(), vec![2u8, 24, 1, 4, 10, 1, 2, 0]);
    assert_eq!(
        Wire::Network { cidr: true }.decode(&buffer[..]),
        Ok("10.1.2.0/24".to_owned())
    );

    let mut buffer = BytesMut::new();
    SqlValParam(&SqlVal::Text("192.168.1.5".to_owned()))
        .to_sql_checked(&Type::INET, &mut buffer)
        .unwrap();
    assert_eq!(
        Wire::Network { cidr: false }.decode(&buffer[..]),
        Ok("192.168.1.5".to_owned())
    );
}

#[test]
fn extension_enum_and_domain_columns_are_recognised() {
    let citext = synthetic("citext", 65_535, Kind::Simple);
    assert_eq!(wire_of(&citext), Some(Wire::Text));

    let labels = vec!["happy".to_owned(), "sad".to_owned()];
    let mood = synthetic("mood", 65_534, Kind::Enum(labels));
    assert_eq!(wire_of(&mood), Some(Wire::Text));

    let email = synthetic("email", 65_533, Kind::Domain(Type::TEXT));
    assert_eq!(wire_of(&email), Some(Wire::Text));

    let amount = synthetic("amount", 65_532, Kind::Domain(Type::NUMERIC));
    assert_eq!(wire_of(&amount), Some(Wire::Numeric));

    assert_eq!(wire_of(&Type::INET), Some(Wire::Network { cidr: false }));
    assert_eq!(wire_of(&Type::CIDR), Some(Wire::Network { cidr: true }));
    assert_eq!(wire_of(&Type::INTERVAL), None);
}

#[test]
fn a_textual_column_accepts_its_string_representation() {
    let citext = synthetic("citext", 65_535, Kind::Simple);
    let mut buffer = BytesMut::new();
    assert!(matches!(
        SqlValParam(&SqlVal::Text("Mixed Case".to_owned()))
            .to_sql_checked(&citext, &mut buffer)
            .unwrap(),
        IsNull::No
    ));
    assert_eq!(buffer.to_vec(), b"Mixed Case".to_vec());
}

#[test]
fn a_value_without_a_textual_form_is_rejected_for_a_network_column() {
    let mut buffer = BytesMut::new();
    let error = match SqlValParam(&SqlVal::I64(1)).to_sql_checked(&Type::INET, &mut buffer) {
        Ok(_) => panic!("an integer has no textual network address form"),
        Err(error) => error,
    };

    assert!(error.to_string().contains("int64"));
    assert!(error.to_string().contains("inet"));
}

#[test]
fn an_array_column_binds_one_value_per_element() {
    let mut buffer = BytesMut::new();
    let param = SqlValParam(&SqlVal::Array(vec![SqlVal::I64(1), SqlVal::I64(2)]));
    assert!(matches!(
        param
            .to_sql_checked(&Type::INT8_ARRAY, &mut buffer)
            .unwrap(),
        IsNull::No
    ));

    // One dimension, no NULL, the `int8` element OID, two elements from the first
    // lower bound, and then the payload of every element.
    let mut expected = Vec::new();
    for field in [1i32, 0, 20, 2, 1] {
        expected.extend_from_slice(&field.to_be_bytes());
    }
    for value in [1i64, 2i64] {
        expected.extend_from_slice(&8i32.to_be_bytes());
        expected.extend_from_slice(&value.to_be_bytes());
    }

    assert_eq!(buffer.to_vec(), expected);
}

#[test]
fn an_array_round_trips_through_the_binary_form() {
    let values = vec![SqlVal::Text("a".to_owned()), SqlVal::Null];

    let mut buffer = BytesMut::new();
    SqlValParam(&SqlVal::Array(values.clone()))
        .to_sql_checked(&Type::TEXT_ARRAY, &mut buffer)
        .unwrap();

    assert_eq!(
        decode_value(&Type::TEXT_ARRAY, &buffer[..]).expect("the array is decodable"),
        SqlVal::Array(values)
    );
}

#[test]
fn arrays_of_enums_and_domains_decode_their_elements() {
    let moods = array_of(
        "_mood",
        65_530,
        synthetic(
            "mood",
            65_534,
            Kind::Enum(vec!["happy".to_owned(), "sad".to_owned()]),
        ),
    );
    let mut buffer = BytesMut::new();
    SqlValParam(&SqlVal::Array(vec![SqlVal::Text("happy".to_owned())]))
        .to_sql_checked(&moods, &mut buffer)
        .unwrap();
    assert_eq!(
        decode_value(&moods, &buffer[..]).expect("the enum array is decodable"),
        SqlVal::Array(vec![SqlVal::Text("happy".to_owned())])
    );

    let amounts = array_of(
        "_amount",
        65_531,
        synthetic("amount", 65_532, Kind::Domain(Type::NUMERIC)),
    );
    let mut buffer = BytesMut::new();
    SqlValParam(&SqlVal::Array(vec![SqlVal::Text("12.5".to_owned())]))
        .to_sql_checked(&amounts, &mut buffer)
        .unwrap();
    let decoded = decode_value(&amounts, &buffer[..]).expect("the decimal array is decodable");
    #[cfg(feature = "decimal")]
    assert_eq!(
        decoded,
        SqlVal::Array(vec![SqlVal::Decimal(rust_decimal::Decimal::new(125, 1))])
    );
    #[cfg(not(feature = "decimal"))]
    assert_eq!(
        decoded,
        SqlVal::Array(vec![SqlVal::Text("12.5".to_owned())])
    );
}

#[test]
fn an_empty_array_decodes_without_elements() {
    let mut bytes = Vec::new();
    for field in [0i32, 0, 20] {
        bytes.extend_from_slice(&field.to_be_bytes());
    }

    assert_eq!(
        decode_value(&Type::INT8_ARRAY, &bytes).expect("an empty array is decodable"),
        SqlVal::Array(Vec::new())
    );
}

#[test]
fn a_multi_dimensional_array_is_reported() {
    let mut bytes = Vec::new();
    for field in [2i32, 0, 20] {
        bytes.extend_from_slice(&field.to_be_bytes());
    }

    let error =
        decode_value(&Type::INT8_ARRAY, &bytes).expect_err("a matrix cannot decode into a vector");

    assert!(error.describe().contains("2 dimensions"));
}

#[test]
fn a_scalar_value_is_rejected_for_an_array_column() {
    let mut buffer = BytesMut::new();
    let error = match SqlValParam(&SqlVal::I64(1)).to_sql_checked(&Type::INT8_ARRAY, &mut buffer) {
        Ok(_) => panic!("a scalar cannot fill an array column"),
        Err(error) => error,
    };

    assert!(error.to_string().contains("int64"));
}

#[cfg(all(feature = "uuid", feature = "chrono", feature = "serde_json"))]
#[test]
fn uuid_json_and_temporal_values_bind_directly() {
    let mut buffer = BytesMut::new();
    let uuid_param = SqlValParam(&SqlVal::Uuid(uuid::Uuid::nil()));
    assert!(matches!(
        uuid_param.to_sql(&Type::UUID, &mut buffer).unwrap(),
        IsNull::No
    ));

    let mut buffer = BytesMut::new();
    let json_param = SqlValParam(&SqlVal::Json(serde_json::json!({ "a": 1 })));
    assert!(matches!(
        json_param.to_sql(&Type::JSONB, &mut buffer).unwrap(),
        IsNull::No
    ));

    let mut buffer = BytesMut::new();
    let timestamp = chrono::NaiveDate::from_ymd_opt(2024, 1, 2)
        .unwrap()
        .and_hms_opt(3, 4, 5)
        .unwrap();
    let timestamp_param = SqlValParam(&SqlVal::NaiveDateTime(timestamp));
    assert!(matches!(
        timestamp_param
            .to_sql(&Type::TIMESTAMP, &mut buffer)
            .unwrap(),
        IsNull::No
    ));

    let mut buffer = BytesMut::new();
    let date = chrono::NaiveDate::from_ymd_opt(2024, 1, 2).unwrap();
    let date_param = SqlValParam(&SqlVal::NaiveDate(date));
    assert!(matches!(
        date_param.to_sql(&Type::DATE, &mut buffer).unwrap(),
        IsNull::No
    ));
}

#[cfg(feature = "decimal")]
#[test]
fn a_numeric_column_binds_an_exact_decimal() {
    let value = rust_decimal::Decimal::new(12_345_678, 3);

    let mut buffer = BytesMut::new();
    assert!(matches!(
        SqlValParam(&SqlVal::Decimal(value))
            .to_sql_checked(&Type::NUMERIC, &mut buffer)
            .unwrap(),
        IsNull::No
    ));

    assert_eq!(
        decode_value(&Type::NUMERIC, &buffer[..]).expect("the numeric value is decodable"),
        SqlVal::Decimal(value)
    );
}

#[cfg(feature = "decimal")]
#[test]
fn an_exact_decimal_binds_an_integer_column_at_its_width() {
    let value = rust_decimal::Decimal::new(75, 1);

    let mut buffer = BytesMut::new();
    SqlValParam(&SqlVal::Decimal(value))
        .to_sql_checked(&Type::INT2, &mut buffer)
        .unwrap();
    assert_eq!(buffer.to_vec(), vec![0u8, 7]);

    let mut buffer = BytesMut::new();
    SqlValParam(&SqlVal::Decimal(value))
        .to_sql_checked(&Type::INT8, &mut buffer)
        .unwrap();
    assert_eq!(buffer.to_vec(), 7i64.to_be_bytes().to_vec());
}

#[cfg(feature = "decimal")]
#[test]
fn an_exact_decimal_out_of_the_range_of_an_integer_column_is_rejected() {
    let mut buffer = BytesMut::new();
    let error = match SqlValParam(&SqlVal::Decimal(rust_decimal::Decimal::MAX))
        .to_sql_checked(&Type::INT8, &mut buffer)
    {
        Ok(_) => panic!("the value does not fit a bigint column"),
        Err(error) => error,
    };

    assert!(error.to_string().contains("out of the range"));
    assert!(error.to_string().contains("int8"));
}

#[cfg(feature = "decimal")]
#[test]
fn an_exact_decimal_binds_a_real_column() {
    let mut buffer = BytesMut::new();
    SqlValParam(&SqlVal::Decimal(rust_decimal::Decimal::new(15, 1)))
        .to_sql_checked(&Type::FLOAT4, &mut buffer)
        .unwrap();

    assert_eq!(buffer.to_vec(), 1.5f32.to_be_bytes().to_vec());
}

#[cfg(feature = "decimal")]
#[test]
fn an_exact_decimal_binds_a_textual_column() {
    let mut buffer = BytesMut::new();
    SqlValParam(&SqlVal::Decimal(rust_decimal::Decimal::new(1500, 3)))
        .to_sql_checked(&Type::TEXT, &mut buffer)
        .unwrap();

    assert_eq!(buffer.to_vec(), b"1.500".to_vec());
}

#[cfg(all(feature = "uuid", feature = "chrono", feature = "serde_json"))]
#[test]
fn text_parameters_bind_to_uuid_json_and_temporal_types() {
    let mut buffer = BytesMut::new();
    let uuid_param = SqlValParam(&SqlVal::Text(
        "00000000-0000-0000-0000-000000000000".to_owned(),
    ));
    assert!(matches!(
        uuid_param.to_sql(&Type::UUID, &mut buffer).unwrap(),
        IsNull::No
    ));

    let mut buffer = BytesMut::new();
    let json_param = SqlValParam(&SqlVal::Text("{\"a\":1}".to_owned()));
    assert!(matches!(
        json_param.to_sql(&Type::JSONB, &mut buffer).unwrap(),
        IsNull::No
    ));

    let mut buffer = BytesMut::new();
    let timestamp_param = SqlValParam(&SqlVal::Text("2024-01-02 03:04:05".to_owned()));
    assert!(matches!(
        timestamp_param
            .to_sql(&Type::TIMESTAMP, &mut buffer)
            .unwrap(),
        IsNull::No
    ));

    let mut buffer = BytesMut::new();
    let date_param = SqlValParam(&SqlVal::Text("2024-01-02".to_owned()));
    assert!(matches!(
        date_param.to_sql(&Type::DATE, &mut buffer).unwrap(),
        IsNull::No
    ));
}
