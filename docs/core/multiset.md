# The MULTISET statement

A nested collection is not stitched together on the client: it becomes one aggregate subquery inside the statement.

## The shape of the statement

PostgreSQL aggregates rows natively, so a collection is `jsonb_agg(to_jsonb(...))` over the nested query, with an empty array for a parent that has no children:

```sql
SELECT "id",
       COALESCE((SELECT jsonb_agg(to_jsonb("__rurqe_sub"))
                 FROM (SELECT "order_id" AS "order_id", "total" AS "total"
                       FROM "orders") AS "__rurqe_sub"), '[]'::jsonb)
FROM "users"
```

Every child row is aggregated by the engine, so the ordering, the filter, and the row limit of the nested query decide the contents. The parameters of the nested query continue the placeholder numbering of the outer statement, which is why a filter in the outer query and one in the inner query never collide.

## SQLite: the same document, assembled field by field

SQLite has no native row aggregation, so a dialect without it builds every object with `json_object` and collects the objects with `json_group_array`:

```sql
COALESCE((SELECT json_group_array(json_object('order_id', "__rurqe_sub"."order_id",
                                               'total', "__rurqe_sub"."total"))
          FROM (SELECT "order_id" AS "order_id", "total" AS "total" FROM "orders")
               AS "__rurqe_sub"), '[]')
```

Both forms produce the same JSON document, so the client decodes one representation regardless of the engine that produced it.

## `nest_one`: one row, no `COALESCE`

The single-row form differs in two details: the aggregate is not wrapped in `COALESCE`, and the nested statement is limited to one row.

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
