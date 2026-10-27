# JSON and JSONB

## `PgJson`

PostgreSQL prints a JSON document as text and accepts it in that form, so a
schema maps a `json` or `jsonb` column onto a lightweight wrapper without
pulling in a serialisation crate:

```rust
pub struct PgJson<T = String>(pub T);

impl<T: Into<String>> ToSqlVal for PgJson<T> {
    fn to_sql_val(self) -> SqlVal {
        SqlVal::Text(self.0.into())
    }
}
```

* `PgJson` binds as `SqlVal::Text`, and so does a borrowed `&PgJson<T>`.
* `MakeNullable` lifts it into `Option<PgJson<T>>` for outer-join
  projections.
* Under the crate's `serde_json` feature, `serde_json::Value` and the core's
  `Json<T>` are JSON value kinds as well.

## Which columns carry the operators

`PgJsonValue` marks the value kinds the JSON operators read: `PgJson<T>`,
`Option<PgJson<T>>`, and — under `serde_json` — `serde_json::Value` and
`Json<T>`, each plain or optional. A text column is not a JSON document:

```rust,compile_fail
// Fails to compile: `content` holds text, not a JSON document.
let _ = DocContent.json_contains("{\"a\": 1}");
```

## Operators

| Method | SQL | Reads |
|---|---|---|
| `json_contains(other)` | `"col" @> $1` | the column document contains `other` |
| `json_has_key(key)` | `"col" ? $1` | a top-level key exists |
| `json_has_any_keys(keys)` | `"col" ?\| $1` | binds **one array parameter** |
| `json_get(key)` | `"col" -> $1` | value at an object key or array index |
| `json_get_text(key)` | `"col" ->> $1` | same, scalar rendered as text |

An object key binds as a text parameter; an array index renders as an
**integer literal** — `"metadata" -> 0`, `"metadata" ->> -1`.

### Why `json_has_any_keys` binds one array parameter

`jsonb ?\| text[]` reads an array operand, so every key binds as one element
of a *single* array parameter (`SqlVal::Array`). Binding the keys as a text
literal would have the driver describe the parameter as `text`, which
PostgreSQL rejects for the `text[]` operand of the operator.

### Why an array index renders as a literal

The statement is sent without parameter type hints, so an untyped parameter
of `->` / `->>` resolves against both the `text` and the `integer` overload
of the operator, and the server types the unknown argument as `text` — the
adapter then refuses to encode the `i32` index for a text operand at bind
time. A literal carries its `integer` type to the server, which selects the
`integer` overload before any parameter is encoded. The literal is built
from an `i32`, so it holds nothing but digits and a sign; a negative index
renders verbatim.

## `JsonKey`

`json_get` and `json_get_text` accept `impl Into<JsonKey>`:

```rust
pub enum JsonKey {
    Key(String),   // From<&str>, From<String>
    Index(i32),   // From<i32> — zero-based array index
}
```

## Nested collections

With `SUPPORTS_JSONB_ROW_AGG` the dialect aggregates nested row sets
natively as `jsonb_agg(to_jsonb(…))` — the `nest_many` / `nest_one`
projections keep the ordering, filter, and row limit of the nested
statement in charge of the produced document.
