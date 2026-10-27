# JSON and JSONB

PostgreSQL prints a JSON document as text and accepts it in that form, so a
`json` or `jsonb` column maps onto the `PgJson` wrapper without pulling a
serialisation crate into the schema:

```rust
use rurqe::postgres::prelude::*;

table! {
    /// The `docs` table.
    Doc => "docs" {
        DocId => "id": i64,
        DocMetadata => "metadata": PgJson,
    }
}
```

`PgJson<T = String>` binds as text, `MakeNullable` lifts it into
`Option<PgJson<T>>` for outer-join projections, and under the crate's
`serde_json` feature `serde_json::Value` and the core `Json<T>` are JSON value
kinds as well.

## Query inside the document

```rust
let drafts = conn.fetch_all(&select_from(Doc)
    .select(select!(
        doc_id: DocId,
        owner: DocMetadata.json_get_text("owner"),
    ))
    .where_(DocMetadata.json_get_text("status").eq("draft"))
    .order_by_asc(DocId))?;

for draft in drafts {
    println!("#{} owned by {:?}", draft.doc_id, draft.owner);
}
```

```sql
SELECT "id" AS "doc_id", ("metadata" ->> $1) AS "owner" FROM "docs"
WHERE (("metadata" ->> $2) = $3)
ORDER BY "id" ASC
```

No cast, no `::text`, no raw SQL: the extraction is an ordinary expression, so
it appears in a projection, a filter, an ordering, or a `CASE` branch.

## Operators

| Method | SQL | Reads |
|---|---|---|
| `json_contains(other)` | `"metadata" @> $1` | the document contains `other` |
| `json_has_key(key)` | `"metadata" ? $1` | a top-level key exists |
| `json_has_any_keys(keys)` | `"metadata" ?\| $1` | one of the keys exists — one array parameter |
| `json_get(key)` | `"metadata" -> $1` | the value at a key or an index |
| `json_get_text(key)` | `"metadata" ->> $1` | the same value rendered as text |

```rust
// A document that carries the flag and one of two keys
select_from(Doc)
    .where_(DocMetadata.json_contains(r#"{"active": true}"#))
    .where_(DocMetadata.json_has_any_keys(&["owner", "editor"]));
```

## `JsonKey`: a key or an index

`json_get` and `json_get_text` accept `impl Into<JsonKey>`:

```rust
pub enum JsonKey {
    Key(String),   // From<&str>, From<String>
    Index(i32),    // From<i32> — zero-based array index
}
```

```rust
DocMetadata.json_get("owner");   // ("metadata" -> $1)
DocMetadata.json_get(0i32);      // ("metadata" -> 0)
```

## Two details worth knowing

* **An object key binds as a parameter**, which the server types as the `text`
  operand of the operator.
* **An array index renders as an integer literal.** The statement is sent
  without parameter type hints, so an untyped parameter of `->` / `->>`
  resolves against both the `text` and the `integer` overload and the server
  types the unknown argument as `text`; the adapter then refuses to encode the
  `i32` index for a text operand. A literal carries its `integer` type, so the
  server selects the `integer` overload before any parameter is encoded. The
  literal is built from an `i32`, so it holds nothing but digits and a sign — a
  negative index renders verbatim.

`json_has_any_keys` binds the keys as **one** array parameter (`SqlVal::Array`)
because `jsonb ?| text[]` reads an array operand; a text literal would be
described as `text` and rejected for that operand.

## Compiler Guard in Action

A text column is not a JSON document, so the operators are simply not there:

```rust,compile_fail
// Fails to compile: `content` holds text, not a JSON document.
let _ = DocContent.json_contains("{\"a\": 1}");
```

* A column declared as `String` has no `json_get` / `json_contains` at all.
* The diagnostic names the value kinds that do carry the operators: `PgJson`,
  `Option<PgJson>`, and — with the `serde_json` feature — `serde_json::Value`
  and `Json<T>`.

## Nested collections

With native JSON row aggregation a nested row set becomes
`jsonb_agg(to_jsonb(…))`, which keeps the ordering, filter, and row limit of the
nested statement in charge of the produced document — see
[Nested results](../../features/nested.md).
