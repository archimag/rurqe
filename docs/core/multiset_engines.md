# Engines, capability gates, and decoding

## `SupportsJsonAggregation`

```text
#[diagnostic::on_unimplemented(
    message = "dialect `{Self}` does not support JSON aggregation of a row set",
    label = "JSON array and object aggregation is not supported by this dialect",
    note = "nested collections and objects require a dialect that implements `SupportsJsonAggregation` (such as PostgreSQL or SQLite 3.38+)"
)]
pub trait SupportsJsonAggregation: Dialect {}
```

The projection itself states the requirement: `QueryD: SupportsJsonAggregation` in `SelectTargetInScope<Tables, At<0>, QueryD>` for `NestMany` and `NestOne`. A dialect without this capability never receives SQL it cannot parse.

## Rendering form and versions

| Dialect | `SUPPORTS_JSONB_ROW_AGG` | Form |
|---|---|---|
| `Postgres<V>` | `true` | `jsonb_agg(to_jsonb(...))` / `to_jsonb(...)` |
| `Sqlite<Sqlite3_38>` | `false` | `json_group_array(json_object(...))` / `json_object(...)` |

The JSON functions of SQLite were an optional build extension before 3.38.0, so the marker is implemented for `Sqlite<Sqlite3_38>`, while `Sqlite<Sqlite3_35>` does not implement it and does not accept a nested projection. The default of `Dialect::SUPPORTS_JSONB_ROW_AGG` is `false`: a dialect declares native row aggregation explicitly.

## Dialect binding

```text
InnerD: Dialect + DialectInScope<QueryD>
```

The nested statement belongs to the same dialect as the enclosing query (or is dialect-neutral), so a PostgreSQL collection is not nested into a SQLite statement.

## Decoding

PostgreSQL reports a nested row document as a JSON value (`SqlVal::Json`), while SQLite reports it as text (`SqlVal::Text`). `json_object` accepts both forms; `nested_field` reads the fields, decoding each one through the same `FromSqlValue` contract a column of the same type uses. The collection is assembled through `SqlArrayElement`: `Vec<Row>` and `Option<Row>` decode from the document without manual JSON parsing.

| Value type | Representation in the document |
|---|---|
| `i32`, `i64` | number |
| `f64` | number; `NaN` and the infinities become `null` |
| `bool` | boolean |
| `String` | string |
| `Vec<u8>` | canonical hex text that `FromSqlValue for Vec<u8>` reads back |
| `Decimal` (feature `decimal`) | number, when every digit is preserved, otherwise text |
| `Uuid`, `NaiveDateTime`, `DateTime<Utc>`, `NaiveDate` | `Display` text that `FromSqlValue` parses back |
| nested JSON | as is |

## Reverse direction

`multiset_object(&[(&str, SqlVal)])` assembles a row document from the values of its fields, and `sql_val_to_json` converts a SQL value into JSON. This is how a row document is bound as a parameter and read back through the same keys.

## The `serde_json` feature

Without the `serde_json` feature there is no document: `RowObject` collapses into an empty struct, `json_object` returns `RowDecodeError::InvalidValue` with a detail about the missing feature, and `multiset_object` returns `SqlVal::EncodingError`. The functions stay callable — that is what lets the `dto!` and `select!` expansions compile in any build, while the reason is reported at the decode site.

## Requirement summary

| Requirement | Checked in |
|---|---|
| The dialect of the nested query matches the query dialect | `DialectInScope<QueryD>`, compilation |
| The dialect can aggregate a row set into JSON | `SupportsJsonAggregation`, compilation |
| The row type declares the document keys | `MultisetRow::FIELDS`, compilation |
| The nested query projects rows | `HasProjection`, compilation |
| Raw-fragment parameters match the placeholders | `BuildError::RawSql`, pre-flight validation in `.build()` |
