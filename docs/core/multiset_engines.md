# Engines, capability gates, and decoding

## The dialect requirement

The projection exists only for a dialect that can aggregate a set of rows into a JSON document: PostgreSQL always, SQLite from 3.38. The JSON functions of SQLite were an optional build extension before that release, so a default build of an older version would reject the statement at runtime — the projection is refused at compile time instead, and the compiler reports that the dialect does not support JSON aggregation of a row set. A dialect that cannot aggregate a row set never receives SQL it cannot parse.

## Rendering form and versions

| Dialect | Form |
|---|---|
| `Postgres<V>` | `jsonb_agg(to_jsonb(...))` / `to_jsonb(...)` |
| `Sqlite<Sqlite3_38>` | `json_group_array(json_object(...))` / `json_object(...)` |

Nested rendering is never emulated: a dialect declares the native aggregation form it supports, and a dialect that declares none never receives a nested projection in a form its engine would reject.

## Dialect binding

The nested statement belongs to the same dialect as the enclosing query (or is dialect-neutral), so a PostgreSQL collection is not nested into a SQLite statement.

## Decoding

PostgreSQL reports a nested row document as a JSON value (`SqlVal::Json`), while SQLite reports it as text (`SqlVal::Text`). The decoder accepts both forms, and the collection — a list of child rows or a single optional child row — is assembled from the document without manual JSON parsing. The keys of the document are the field names of the nested projection.

| Value type | Representation in the document |
|---|---|
| `i32`, `i64` | number |
| `f64` | number; `NaN` and the infinities become `null` |
| `bool` | boolean |
| `String` | string |
| `Vec<u8>` | canonical hex text that the byte-column decoding reads back |
| `Decimal` (feature `decimal`) | number, when every digit is preserved, otherwise text |
| `Uuid`, `NaiveDateTime`, `DateTime<Utc>`, `NaiveDate` | `Display` text that the decoder parses back |
| nested JSON | as is |

## The `serde_json` feature

Without the `serde_json` feature there is no document: reading a nested field ends in a typed decoding error that names the missing feature, and binding a row document ends in a typed encoding error. The projections stay callable — that is what lets the `dto!` and `select!` expansions compile in any build, while the reason is reported at the decode site.
