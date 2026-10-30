# The MULTISET statement

A nested collection is not stitched together on the client: it becomes one aggregate subquery inside the statement, and the parent row carries it as a value of its own.

## The shape of the statement

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

## The collection is a named field

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

`user.orders` is a `Vec` of a record with `order_id` and `total`, and `user.user_id` is the parent key. Nothing is decoded by index: the statement is the one the tuple projection produced, with the two output names its fields carry.

## SQLite: the same document, assembled field by field

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

## `nest_one`: one row, no `COALESCE`

The single-row form differs in two details: the aggregate is not wrapped in `COALESCE`, and the nested statement is limited to one row.

```rust,no_run
{{#rustdoc_include ../examples/nested.rs:single}}
```

```sql
SELECT "id", (SELECT to_jsonb("__rurqe_sub")
              FROM (SELECT "total" AS "total" FROM "orders" LIMIT 1) AS "__rurqe_sub")
FROM "users"
```

When the nested statement combines statements (`UNION`, `INTERSECT`, `EXCEPT`), the limit is applied to the combined rows, so the compound statement is the one that is limited.

## Multiset, not set

- `Vec<Row>` keeps the ordering the nested `ORDER BY` asked for and keeps duplicates: SQL without `DISTINCT` does not remove rows.
- `nest_one` reads as `Option<Row>`: a missing child is `None`, not an empty collection.
- `DISTINCT` and `DISTINCT ON` belong to the nested statement itself, where they are called on the query before it is nested — they are not a property of the projection.

## Raw SQL inside a nested query

A `sql!` / `raw_sql!` template is censused where the macro expands, so a literal template whose placeholders disagree with the parameters it is given is refused before it becomes part of a statement. A raw expression assembled from fragments without the macro — including one written inside a nested query — is caught by the pre-flight validation of the enclosing statement, which reports the mismatch as `BuildError::RawSql` before any SQL is rendered, so a value never binds into a lost or shifted slot.

## Which engines can render it

The projection requires the dialect to declare JSON aggregation. PostgreSQL does; SQLite does from 3.38 (`Sqlite3_38`), because the JSON functions were an opt-in extension before that release.
