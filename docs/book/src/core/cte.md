# Common table expressions (`WITH`)

A report that needs one number per user — how much each user has spent in total — states the aggregate once and then joins it to the users. Written as a **common table expression**, that aggregate is named inside the statement and read back as a relation of the same statement: it is computed once, the statement stays readable, and nothing leaves the database.

## A practical walkthrough

The example builds one statement in two steps. The first step aggregates the orders and names the result; the second step reads it like any other relation.

```rust,no_run
# extern crate rurqe;
# use rurqe::postgres::prelude::*;
# table! { User => "users" { UserId => "id": i64, UserName => "name": String, } }
# table! { Order => "orders" { OrderId => "id": i64, OrderAmount => "amount": f64, OrderUserId => "user_id": i64, } }
# table! { UserTotals => "user_totals" { TotalsOwnerId => "owner_id": i64, TotalsTotal => "total": f64, } }
# fn main() {
{{#rustdoc_include ../../../../examples/cte.rs:summary}}
{{#rustdoc_include ../../../../examples/cte.rs:report}}
# }
```

The statement above renders as one round trip:

```text
WITH "user_totals" AS (
    SELECT "user_id" AS "owner_id", SUM("amount") AS "total"
    FROM "orders" GROUP BY "user_id"
)
SELECT "users"."name" AS "user_name", "user_totals"."total" AS "total_spent"
FROM "users" INNER JOIN "user_totals" ON ("user_totals"."owner_id" = "users"."id")
```

### Step one: the aggregate

`.group_by(OrderUserId)` collapses the orders into one row per user, and `.select(cols! { ... })` states what that row carries: the owner of the orders and the total amount. The field names are the column names of the relation — `owner_id` and `total` — so the releasing step reads them under exactly those names.

`.as_cte::<UserTotals>()` turns the finished aggregate into the named relation `user_totals`. The table marker handed to it is what declares the relation: its SQL name becomes the name in the `WITH` clause, and its columns are the ones the rest of the statement reads, with their types.

### Step two: the statement that reads it

`.with(totals)` places the relation in front of the query, and from that point it is part of the scope of the statement. `.inner_join(UserTotals, TotalsOwnerId.eq(UserId))` therefore joins it on the same terms as a stored table — the `ON` predicate compares a column of the relation with a column of `users` — and the projection reads `TotalsTotal` as an ordinary non-nullable `f64`.

Every other clause keeps working unchanged: a filter, an ordering, a second join, or a pagination added to the statement sees the columns of the relation under the names the marker declares.

## What the marker decides

| The marker declares | The statement reads |
|---|---|
| `UserTotals => "user_totals"` | the relation name used by `.with(...)` and by the join |
| `TotalsOwnerId => "owner_id": i64` | the `"owner_id"` column, projected as `i64` |
| `TotalsTotal => "total": f64` | the `"total"` column, projected as `f64` |

Those column names are the ones the inner statement projects, which is why the projection fields of the aggregate carry them. Naming a column the aggregate does not project asks the statement for a column that does not exist, so the two halves are written together.

## Derived tables and `LATERAL`

A finished statement also participates in a query without being given a name of its own, and — on dialects that support it — can be evaluated once per row of the relation it is joined with:

| What you need | What you write |
|---|---|
| a named statement the query reads (`WITH`) | `.as_cte::<Marker>()` and `.with(...)` |
| a statement used as a relation in the join list | `.as_table::<Marker>()` and `.inner_join_subquery(...)` / `.left_join_subquery(...)` |
| a statement evaluated for every row it is joined with | `.as_lateral::<Marker>()` and `.cross_join_lateral(...)` / `.left_join_lateral(...)` |

A derived table renders inline as `(SELECT ...) AS "name"`, so the joined relation behaves like a stored one while its rows come from the statement; a left-joined derived table lifts its columns to `Option<T>` exactly like a left-joined stored table. `LATERAL` is a dialect capability: PostgreSQL supports it, SQLite does not, so the methods do not exist there at compile time. The join side of both is covered in [Joins](joins.md), and the correlated reference they are written with in [Subqueries](subqueries.md).

## Related chapters

- [Joins](joins.md) — derived tables, lateral joins, and the nullability of outer joins.
- [`GROUP BY` and `HAVING`](group_by.md) — the aggregate an expression like this one starts from.
- [Subqueries](subqueries.md) — scalar, `EXISTS`, and `IN` subqueries.
