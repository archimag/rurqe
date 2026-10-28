# Ecosystem types, values, and decoding

Every value passes through one type — `SqlVal`. It is also the exchange unit
between the core and the drivers: parameters reach an adapter as `Vec<SqlVal>`,
rows come back as `Vec<Vec<SqlVal>>`.

## `SqlVal`

```rust
pub enum SqlVal {
    Null,
    I32(i32),
    I64(i64),
    F64(f64),
    Decimal(rust_decimal::Decimal),  // feature = "decimal"
    Bool(bool),
    Text(String),
    Bytes(Vec<u8>),
    Array(Vec<SqlVal>),
    Uuid(uuid::Uuid),                // feature = "uuid"
    NaiveDateTime(chrono::NaiveDateTime), // feature = "chrono"
    DateTimeUtc(chrono::DateTime<chrono::Utc>), // feature = "chrono"
    NaiveDate(chrono::NaiveDate),    // feature = "chrono"
    Json(serde_json::Value),         // feature = "serde_json"
    EncodingError(String),
}
```

The enum is marked `#[non_exhaustive]`, because some variants exist only with
the corresponding Cargo feature enabled. Consequence for application code: a
`match` on `SqlVal` outside the crate must carry a wildcard arm — it stays
correct under any feature set.

`SqlVal::EncodingError` is not cosmetic: when a value cannot be encoded (for
instance a `usize` that does not fit into an `i64`), the parameter is not
replaced by `NULL` — it carries the reason for the refusal, and the driver
returns it to the caller.

## Encoding parameters: `ToSqlVal`

Implementations are provided for `i16`, `i32`, `u32`, `i64`, `usize`, `f64`,
`bool`, `String`, `&str`, `&String`, `Vec<u8>`, `&[u8]`, `SqlVal`, `Option<T>`,
and for the gated families: `rust_decimal::Decimal` and `&Decimal`, `uuid::Uuid`
and `&Uuid`, `chrono::NaiveDateTime`, `chrono::DateTime<Utc>`,
`chrono::NaiveDate`, `serde_json::Value`, `Json<T>` and `&Json<T>`.

Details fixed by the code:

- `i16` encodes as `SqlVal::I32`, `u32` as `SqlVal::I64`;
- `usize` tries to become an `i64`, otherwise it yields an `EncodingError` with
  the text "the value … cannot be represented as a 64-bit signed integer";
- `Option<T>::None` is `SqlVal::Null`;
- `Json<T>` is serialized through `serde`; a serialization failure becomes an
  `EncodingError`, never a panic.

## Decoding values: `FromSqlValue`

Every `FromSqlValue` implementation is paired with a `ToSqlVal` one, so a value
round-trips from a parameter into a result row without an intermediate text
form.

Decoding is deliberately tolerant where the engine has no separate storage
class:

| Target type | Accepts |
|---|---|
| `i16`, `i32`, `u32`, `usize` | `I32` and `I64` within range |
| `i64` | `I64`; `Decimal` (if it fits); `F64` without a fractional part |
| `f64` | `F64`, `I32`, `I64`, `Decimal` |
| `bool` | `Bool`, and the integers `0` / `1` |
| `String` | `Text`, and the textual forms of `Uuid`, temporal types, `Json`, `Decimal` |
| `Vec<u8>` | `Bytes`, and `Text` in hexadecimal form (an `\x` prefix is allowed, case is irrelevant) |
| `Decimal` | `Decimal`, `I32`, `I64`, `F64`, `Text` |
| `uuid::Uuid` | `Uuid`, `Text`, `Bytes` |
| `serde_json::Value`, `Json<T>` | `Json`, `Text`, `Bytes` |
| `Option<T>` | `Null` → `None`, otherwise `T::from_sql_value` |

An out-of-range value is a `RowDecodeError::InvalidValue`, not a silent
truncation: `i64` refuses `F64(0.5)`, `NaN`, infinity, or `1e30`. `String`
accepts the textual form of a "domain" value exactly because the code generator
maps such columns to `String` by default.

The hexadecimal exchange for binary values (`hex_encode` / `hex_decode`) exists
for nested JSON documents: a binary field of a nested row arrives as text and
is decoded back into bytes.

## The decode error

```rust
pub enum RowDecodeError {
    TypeMismatch { target: &'static str, found: &'static str },
    InvalidValue { target: &'static str, found: &'static str, detail: String },
    ArityMismatch { expected: usize, actual: usize },
}
```

`target` is the `std::any::type_name` of the target type, `found` is
`SqlVal::variant_name()`. The rendering produces readable messages: "cannot
decode a SQL `text` value into `i64`", "cannot decode a row of 1 values into a
row type of 2 values".

## Rows: `FromRow`

`FromRow` is implemented for tuples of 1 to 12 elements. `try_from_row` checks
the arity **before** decoding values, so a column-count mismatch is never masked
by a type error. `from_row` is a shorthand returning `Option`.

```rust
let row = <(i64, String)>::try_from_row(&[
    SqlVal::I64(1),
    SqlVal::Text("root".to_owned()),
]);
assert_eq!(row, Ok((1, "root".to_owned())));
```

The generated `FromRow` in `schema.rs` never indexes the slice directly: it
reads through `values.get(i)` and reports `RowDecodeError::ArityMismatch` on a
short response.

## Feeding a decode error into execution

`ExecutionError::RowDecode { row_index, source }` carries the index of the first
row that failed to decode. The index is zero when the query requested a single
row through `fetch_optional`/`fetch_one`: at most one row is decoded there.

## Streaming rows and choosing the collection

`fetch_collect` and `fetch_iter` (see [Runtime: overview](overview.md)) change
only *when* a row is decoded, never *how*:

- `fetch_collect::<C, _>` decodes every row into `C: FromIterator<Q::Row>`, so a
  projection of pairs becomes a `HashMap`, a projection of one column becomes a
  `HashSet`, and a `Vec` is one choice among them rather than the only one;
- `fetch_iter` hands back a `QueryRowIter` owning the raw `Vec<Vec<SqlVal>>` of a
  statement that has already finished: the connection is released before the
  first row is decoded, and `FromRow` runs on `.next()`.

A decode failure keeps the same shape in both cases: the index is the position
of the failing row inside the returned buffer and starts at zero.

## What happens on the PostgreSQL side

The adapter encodes a parameter into the binary representation of the **column
type**, not of the value type:

- the domain chain is expanded, so a domain column receives the
  same layout as the type it is declared over;
- integer values are narrowed to `INT2`/`INT4` when the column has that width;
  an out-of-range value is an encoding error;
- `NUMERIC` is encoded exactly, through the textual form of the decimal value
  rather than through `f64`;
- arrays are encoded element by element with the element codec, and a `NULL`
  element stays `NULL`;
- columns without a separate `SqlVal` variant (UUID, JSON/JSONB, temporal types
  with the features off) receive the textual form, which the server parses
  itself;
- `NULL` binds for a column of any type.

During decoding the rule set is classified once per column and reused for every
row of the result; the payload is read from the borrowed row buffer without an
intermediate copy.

## What happens on the SQLite side

`rusqlite` has no separate class for boolean and narrow integers: values arrive
as `I64`/`F64`/`Text`/`Blob`/`Null`. That is why `bool` accepts `0`/`1` and why
`Vec<T>` is encoded as a JSON document (see
[Arrays in SQLite](../dialects/sqlite/overview.md)). A text column must be valid
UTF-8: otherwise the conversion error of that column is returned, and the value
is never replaced with replacement characters.
