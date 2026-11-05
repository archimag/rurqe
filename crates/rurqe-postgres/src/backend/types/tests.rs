//! Regression tests for the binding layer.
//!
//! The tests drive the codecs through the type descriptors a server would report
//! for a column, so the wire representation of a column the code generator maps
//! onto `String` stays pinned without a running database.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

use bytes::BytesMut;
use rurqe_core::expr::SqlVal;
use rurqe_core::query::BuiltQuery;

use super::driver::types::{IsNull, Kind, ToSql, Type};
use super::wire::{wire_of, Wire};
use super::{server_refusal, BoundParams, PgError, ResultRow, RowDecoder, SqlValParam};
// The binding layer sits on the hot path of every statement, so its allocation
// count is observed rather than assumed. The counter is thread-local because
// the test harness runs scenarios concurrently in one process: a thread that
// never armed the counter allocates through the system allocator exactly as it
// did before, so only the measuring test contributes to the count.
thread_local! {
    // Whether the current thread counts its allocations.
    static COUNTING: Cell<bool> = const { Cell::new(false) };
    // How many allocations the current thread made while counting was armed.
    static ALLOCATIONS: Cell<usize> = const { Cell::new(0) };
}

/// A global allocator that counts the allocations of an armed thread.
struct CountingAllocator;

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        record_allocation();
        System.alloc(layout)
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        record_allocation();
        System.alloc_zeroed(layout)
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        record_allocation();
        System.realloc(ptr, layout, new_size)
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        System.dealloc(ptr, layout);
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

/// Counts one allocation of the current thread while counting is armed.
fn record_allocation() {
    let armed = COUNTING
        .try_with(|counting| counting.get())
        .unwrap_or(false);
    if armed {
        let _ = ALLOCATIONS.try_with(|allocations| allocations.set(allocations.get() + 1));
    }
}
/// Builds a type the way the server would report it.
fn synthetic(name: &str, oid: u32, kind: Kind) -> Type {
    Type::new(name.to_owned(), oid, kind, "public".to_owned())
}

/// Runs `f` while counting the allocations it makes on the current thread.
fn allocations_of<T>(f: impl FnOnce() -> T) -> (T, usize) {
    ALLOCATIONS.with(|allocations| allocations.set(0));
    COUNTING.with(|counting| counting.set(true));
    let value = f();
    COUNTING.with(|counting| counting.set(false));
    (value, ALLOCATIONS.with(|allocations| allocations.get()))
}

/// A result row built from synthetic column types and payloads.
struct SyntheticRow<'a> {
    /// Type each column is declared as, paired with its binary payload.
    columns: Vec<(&'a Type, Option<&'a [u8]>)>,
}

impl ResultRow for SyntheticRow<'_> {
    fn column_types(&self) -> impl Iterator<Item = &Type> + '_ {
        self.columns.iter().map(|(column_type, _)| *column_type)
    }

    fn column_payload(&self, index: usize) -> Result<Option<&[u8]>, PgError> {
        match self.columns.get(index) {
            Some((_, payload)) => Ok(*payload),
            None => Ok(None),
        }
    }
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
fn binary_octets_are_rejected_for_a_textual_column() {
    let mut buffer = BytesMut::new();
    let error = match SqlValParam(&SqlVal::Bytes(b"raw".to_vec()))
        .to_sql_checked(&Type::TEXT, &mut buffer)
    {
        Ok(_) => panic!("binary octets cannot fill a text column"),
        Err(error) => error,
    };

    // The octets are valid UTF-8, so the server would accept them as text instead
    // of reporting the mismatch this binding is.
    assert!(error
        .to_string()
        .contains("cannot bind a SQL `bytes` value"));
    assert!(error.to_string().contains("text"));
    assert!(buffer.is_empty());
}

#[test]
fn a_boolean_is_rejected_for_a_textual_column() {
    let mut buffer = BytesMut::new();
    let error = match SqlValParam(&SqlVal::Bool(true)).to_sql_checked(&Type::VARCHAR, &mut buffer) {
        Ok(_) => panic!("a boolean cannot fill a textual column"),
        Err(error) => error,
    };

    // `bool` writes a single octet, which no textual column can read.
    assert!(error
        .to_string()
        .contains("cannot bind a SQL `boolean` value"));
    assert!(error.to_string().contains("varchar"));
    assert!(buffer.is_empty());
}

#[test]
fn an_integer_is_rejected_for_a_binary_column() {
    let mut buffer = BytesMut::new();
    let error = match SqlValParam(&SqlVal::I64(1)).to_sql_checked(&Type::BYTEA, &mut buffer) {
        Ok(_) => panic!("an integer cannot fill a binary column"),
        Err(error) => error,
    };

    assert!(error
        .to_string()
        .contains("cannot bind a SQL `int64` value"));
    assert!(error.to_string().contains("bytea"));
    assert!(buffer.is_empty());
}

#[test]
fn a_floating_point_value_is_rejected_for_an_integer_column() {
    let mut buffer = BytesMut::new();
    let error = match SqlValParam(&SqlVal::F64(1.5)).to_sql_checked(&Type::INT8, &mut buffer) {
        Ok(_) => panic!("a floating point value cannot fill an integer column"),
        Err(error) => error,
    };

    assert!(error.to_string().contains("float64"));
    assert!(error.to_string().contains("int8"));
    assert!(buffer.is_empty());
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
fn a_statement_prepares_its_parameters_once() {
    let query = BuiltQuery {
        sql: "select 1 where id = $1".to_owned(),
        params: vec![SqlVal::I64(1)],
    };

    let (bound, allocations) = allocations_of(|| {
        let params = BoundParams::of(&query);
        params.bindings().count()
    });

    assert_eq!(bound, 1);
    // The statement pays for one buffer of wrappers. A second buffer of
    // driver references would raise this count and fail the test.
    assert_eq!(
        allocations, 1,
        "binding one parameter must allocate one buffer of its bound values"
    );
}

#[test]
fn a_result_set_is_decoded_one_row_at_a_time() {
    let payload: &[u8] = b"alice";
    let mut decoder = RowDecoder::new();
    let row = SyntheticRow {
        columns: vec![(&Type::TEXT, Some(payload))],
    };

    // The first row pays for the classification of the result set, which every
    // further row reuses.
    let (first, first_allocations) = allocations_of(|| decoder.decode(&row));
    let first = first.expect("the first row of the result set decodes");
    let (second, second_allocations) = allocations_of(|| decoder.decode(&row));
    let second = second.expect("the second row of the result set decodes");

    let expected = vec![SqlVal::Text(String::from("alice"))];
    assert_eq!(first, expected);
    assert_eq!(second, expected);

    // One buffer for the classified result set, one for the decoded row, and one
    // for the text value the row carries.
    assert_eq!(
        first_allocations, 3,
        "decoding the first row must classify the result set once and decode the row"
    );
    // A further row allocates its own decoded values only: the result set is
    // never materialised a second time.
    assert_eq!(
        second_allocations, 2,
        "decoding a further row must not allocate another copy of the result set"
    );
}

#[cfg(feature = "deadpool-postgres")]
#[test]
fn a_pool_failure_stays_reachable_through_its_source() {
    // A checkout failure reaches the adapter as the pool error of the driver,
    // whose own variants separate a backpressure timeout from a backend
    // failure. The adapter wraps it whole, so the cause has to stay reachable
    // through `source()` instead of surviving only in the rendered message.
    let failure = deadpool_postgres::PoolError::Backend(
        "not a connection string"
            .parse::<tokio_postgres::Config>()
            .expect_err("the text is not a connection string"),
    );
    let error = PgError::Pool(failure);

    let source = std::error::Error::source(&error)
        .expect("the wrapped pool failure is the source of the adapter error");

    assert!(
        source.is::<deadpool_postgres::PoolError>(),
        "the source is the pool failure the adapter wrapped"
    );
}

/// A server refusal renders the message the server sent, because the
/// driver's label for a refused statement carries no content at all.
#[test]
fn a_server_refusal_renders_the_message_the_server_sent() {
    assert_eq!(
        server_refusal("column \"view_name\" does not exist", None, None),
        "column \"view_name\" does not exist"
    );
}

/// The detail and hint the server sent follow the message, so the whole
/// refusal reaches the operator.
#[test]
fn detail_and_hint_follow_the_message_the_server_sent() {
    assert_eq!(
        server_refusal("division by zero", Some("a constant folded to zero"), None),
        "division by zero (a constant folded to zero)"
    );
    assert_eq!(
        server_refusal("permission denied", None, Some("connect as the owner")),
        "permission denied; hint: connect as the owner"
    );
    assert_eq!(
        server_refusal("refused", Some("the detail"), Some("the hint")),
        "refused (the detail); hint: the hint"
    );
}
