# Nested Results (MULTISET)

`nest_many` and `nest_one` project a child query as one JSON aggregate subquery inside the enclosing statement. The engine groups the child rows of every parent into a JSON document, and the row decoder converts that document into the row type of the child projection, so the parent rows and their nested rows arrive in a single round trip.

A per-parent fetch issues N+1 statements, and a plain `JOIN` multiplies each parent row by its children and requires client-side folding. The aggregate form avoids both: the children are grouped by the engine.

```rust,no_run
{{#rustdoc_include ../examples/nested.rs:nested}}
```

`user.orders` is a `Vec` of a record with `order_id` and `total`; nothing is decoded by index, and no second statement was sent.

## Nested Query Rendering

The nested statement is a query of its own, so it decides which rows the collection holds. The enclosing statement aggregates them into one JSON document per parent:

```rust,no_run
{{#rustdoc_include ../examples/nested.rs:collection}}
```

```sql
SELECT "id",
       COALESCE((SELECT jsonb_agg(to_jsonb("__rurqe_sub"))
                 FROM (SELECT "id" AS "order_id", "total" AS "total"
                       FROM "orders") AS "__rurqe_sub"), '[]'::jsonb)
FROM "users"
```

PostgreSQL aggregates rows natively, so a collection is `jsonb_agg(to_jsonb(...))` over the nested query, and a parent without children receives an empty array instead of a missing field. The ordering, the filter, and the row limit of the nested query decide what the collection holds, because the engine aggregates the rows that statement produces. The parameters of the nested query continue the placeholder numbering of the outer statement, which is why a filter in the outer query and one in the inner query never collide.

## Named Field Collections

A projection names the collection like any other projected item, so the row exposes it as a field and the client never parses the document by hand:

```rust,no_run
{{#rustdoc_include ../examples/nested.rs:named_collection}}
```

```sql
SELECT "id" AS "user_id",
       COALESCE((SELECT jsonb_agg(to_jsonb("__rurqe_sub"))
                 FROM (SELECT "id" AS "order_id", "total" AS "total"
                       FROM "orders") AS "__rurqe_sub"), '[]'::jsonb) AS "orders"
FROM "users"
```

`user.orders` is a `Vec` of a record with `order_id` and `total`, and `user.user_id` is the parent key. Nothing is decoded by index: the statement is the one the tuple projection produced, with the two output names its fields carry. The outer projection is a `cols!`, because `nest_many(...)` and `nest_one(...)` are computed projections: `dto!` accepts column markers only, and its message points at `select!` for a computed projection.

## Single-Row Results (`nest_one`)

The single-row form reads as an `Option` of the nested projection, so "no row" and "a row without values" stay distinguishable. It differs from the collection in two details: the aggregate is not wrapped in `COALESCE`, and the nested statement is limited to one row.

```rust,no_run
{{#rustdoc_include ../examples/nested.rs:nest_one}}
```

```rust,no_run
{{#rustdoc_include ../examples/nested.rs:single}}
```

```sql
SELECT "id", (SELECT to_jsonb("__rurqe_sub")
              FROM (SELECT "total" AS "total" FROM "orders" LIMIT 1) AS "__rurqe_sub")
FROM "users"
```

A missing child is `None`, not an empty collection. `nest_one` adds the limit of one row inside the nested statement, so the engine never sends the children of one parent to be trimmed afterwards. When the nested statement combines statements (`UNION`, `INTERSECT`, `EXCEPT`), that limit applies to the combined rows.

## Child Query Scope

The nested statement is a query of its own, so its scope holds only its own `FROM`. A column of the outer query enters the inner predicate through `outer(...)`, which marks the expression as resolved by the enclosing statement:

```rust,no_run
{{#rustdoc_include ../examples/nested.rs:correlation}}
```

Without it the child query would reference `"users"`, a relation it never declares, and the call is rejected at the call site.

## JSON Keys and Field Names

The nested `cols!` names the JSON keys of every child row: `cols! { order_id: OrderId, total: OrderTotal }` produces documents of the shape `{"order_id": …, "total": …}`. The declaration and the document cannot drift apart, because both come from the same list.

## Child Query Composition

The filter, the ordering, the row limit, and any grouping of the nested statement decide the contents of the collection — not the client. `DISTINCT` and `DISTINCT ON` belong to the nested statement itself, where they are called on the query before it is nested; they are not a property of the projection.

## Multiset Semantics

- `Vec<Row>` keeps the ordering the nested `ORDER BY` asked for and keeps duplicates: SQL without `DISTINCT` does not remove rows.
- `nest_one` reads as `Option<Row>`: a missing child is `None`, not an empty collection.
- The rows of a collection are ordered by the nested statement, never by the order in which the parent rows arrived.

## Raw SQL inside a nested query
A `sql!` / `raw_sql!` template is checked where the macro expands, so a literal template whose placeholders disagree with the parameters it is given is refused before it becomes part of a statement. A raw expression assembled from fragments without the macro — including one written inside a nested query — is caught by the pre-flight validation of the enclosing statement, which reports the mismatch as `BuildError::RawSql` before any SQL is rendered, so a value never binds into a lost or shifted slot.

## SQLite Rendering

SQLite has no native row aggregation, so a dialect without it builds every object with `json_object` and collects the objects with `json_group_array`:

```rust,no_run
{{#rustdoc_include ../examples/nested.rs:collection_sqlite}}
```

```sql
SELECT "id",
       COALESCE((SELECT json_group_array(json_object('order_id', "__rurqe_sub"."order_id",
                                                     'total', "__rurqe_sub"."total"))
                 FROM (SELECT "id" AS "order_id", "total" AS "total" FROM "orders")
                      AS "__rurqe_sub"), '[]')
FROM "users"
```

Both forms produce the same JSON document, so the client decodes one representation regardless of the engine that produced it. The fragment stops at the assembled statement: it pins a SQLite version and claims the SQL that version renders, without opening a connection.

## Dialect Capability Gates

The projection exists only for a dialect that can aggregate a set of rows into a JSON document: PostgreSQL always, SQLite from 3.38. The JSON functions of SQLite were an optional build extension before that release, so a default build of an older version would reject the statement at runtime — the projection is refused at compile time instead, and the compiler reports that the dialect does not support JSON aggregation of a row set.

| Dialect | Availability | Form |
|---|---|---|
| `Postgres<V>` | every version | `jsonb_agg(to_jsonb(...))` / `to_jsonb(...)` |
| `Sqlite<Sqlite3_38>` | 3.38 and newer | `json_group_array(json_object(...))` / `json_object(...)` |
| SQLite below 3.38 | not available — the projection does not compile | — |

A dialect that cannot aggregate a row set never receives SQL it cannot parse. Nested rendering is never emulated: a dialect declares the native aggregation form it supports, and a dialect that declares none never receives a nested projection in a form its engine would reject. The nested statement belongs to the same dialect as the enclosing query (or is dialect-neutral), so a PostgreSQL collection is not nested into a SQLite statement.

## JSON Document Decoding

PostgreSQL reports a nested row document as a JSON value (`SqlVal::Json`), while SQLite reports it as text (`SqlVal::Text`). The decoder accepts both forms, and the collection — a list of child rows or a single optional child row — is assembled from the document without manual JSON parsing. The keys of the document are the field names of the nested projection, so a declared row type reads them by name:

```rust,no_run
{{#rustdoc_include ../examples/nested.rs:document_row}}
```

The declaration publishes the same keys the projection writes, so the collection decodes into `Vec<OrderLine>` — and a single nested row into `Option<OrderLine>` — without a positional index:

```rust,no_run
{{#rustdoc_include ../examples/nested.rs:document_decode}}
```

A field of a nested row decodes through the same `FromSqlValue` contract a column of the same type decodes through:

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

## Compiler Guard in Action

- A nested statement without a projection (`nest_many(select_from(Order))`) does not compile: there is no row type to collect.
- A dialect without JSON aggregation does not accept the projection at all, so an unsupported engine fails at the type level instead of inside the driver.
- The element type has to declare its keys, which is what the `cols!` record does.
- A nested statement that reads a column of the enclosing query without `outer(...)` is rejected at the call site, because that relation is absent from the child scope.
