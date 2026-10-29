# Common table expressions (`WITH`)

A report that needs one number per user — how much each user has spent in total — states the aggregate once and then joins it to the users. Written as a **common table expression**, that aggregate is named inside the statement and read back as a relation of the same statement: it is computed once, the statement stays readable, and nothing leaves the database.

## Clause anatomy

```sql
WITH name AS (SELECT ...)
    [, name AS (SELECT ...)]
SELECT ...
    FROM relation
    [ [INNER | LEFT | CROSS] JOIN name ON condition ]
    [ WHERE condition ]
    [ GROUP BY expression, ... ]
    [ HAVING condition ]
    [ ORDER BY ... ] [ LIMIT n ] [ OFFSET n ]
```

| Clause | Purpose | Optional | What it changes |
|---|---|---|---|
| `WITH name AS (SELECT ...)` | names a statement inside the query | no | adds a relation the rest of the statement may read |
| the body `(SELECT ...)` | the rows the relation carries | no | fixes the columns and their types, under the names the relation is read by |
| a second `, name AS (...)` | another named statement | yes | adds another relation to the same statement |
| `FROM` / `JOIN name` | reads the relation | yes — a relation nothing reads contributes nothing | joins the named relation exactly like a stored table |
| `LATERAL` | evaluates a subquery once per row it is joined with | yes | turns a subquery into a correlated relation (PostgreSQL) |

A `WITH` is not a second round trip: it is one statement, and a second named relation is simply a second `.with(...)` on the statement that reads it. The shortest useful form names one aggregate and reads it in the same statement — the walkthrough below starts from exactly that chain.

## SQL ↔ DSL mapping

| SQL clause | Builder method | Effect on the statement |
|---|---|---|
| `WITH "user_totals" AS (...)` | `.as_cte::<UserTotals>()` | fixes the relation name and its column names; reachable only on a statement that already records a projection |
| the body `(SELECT ...)` | the finished `select_from(...)...select(...)` handed to `as_cte` | its projection becomes the column list of the relation |
| a second `, name AS (...)` | a second `.with(...)` on the reading statement | adds another relation to the same statement |
| `FROM "users"` | `select_from(User)` / `.from(User)` | introduces the stored relation |
| `JOIN "user_totals" ON ...` | `.inner_join(UserTotals, ...)` / `.left_join(UserTotals, ...)` | makes the named relation part of the scope; a left join lifts its columns to `Option<T>` |
| `(SELECT ...) AS "name"` inline | `.as_table::<Marker>()`, then `.inner_join_subquery(...)` / `.left_join_subquery(...)` | a derived table, read inline without a name of its own |
| `LATERAL` | `.as_lateral::<Marker>()`, then `.cross_join_lateral(...)` / `.left_join_lateral(...)` | evaluated once per row of the relation it is joined with |
| — | `.build()` | renders the statement; a relation name that appears twice is reported here |

## Clause-by-clause walkthrough

### `WITH name AS (...)`: the named statement

```rust,no_run
{{#rustdoc_include ../../../../examples/cte.rs:summary}}
```

```text
SELECT "user_id" AS "owner_id", SUM("amount") AS "total" FROM "orders" GROUP BY "user_id"
```

`.group_by(OrderUserId)` collapses the orders into one row per user, and `.select(cols! { ... })` states what that row carries: the owner of the orders and the total amount. The field names are the column names of the relation — `owner_id` and `total` — so the reading side below addresses them under exactly those names.

`.as_cte::<UserTotals>()` turns the finished aggregate into the named relation `user_totals`. The table marker handed to it is what declares the relation: its SQL name becomes the name in the `WITH` clause, and its columns are the ones the rest of the statement reads, with their types.

### Reading the relation: `FROM` and `JOIN`

```rust,no_run
{{#rustdoc_include ../../../../examples/cte.rs:report}}
```

The statement below renders as one round trip:

```text
WITH "user_totals" AS (
    SELECT "user_id" AS "owner_id", SUM("amount") AS "total"
    FROM "orders" GROUP BY "user_id"
)
SELECT "users"."name" AS "user_name", "user_totals"."total" AS "total_spent"
FROM "users" INNER JOIN "user_totals" ON ("user_totals"."owner_id" = "users"."id")
```

`.with(totals)` places the relation in front of the query, and from that point it is part of the scope of the statement. `.inner_join(UserTotals, TotalsOwnerId.eq(UserId))` therefore joins it on the same terms as a stored table — the `ON` predicate compares a column of the relation with a column of `users` — and the projection reads `TotalsTotal` as an ordinary non-nullable `f64`. `.build()` returns the rendered statement without touching a database.

Every other clause keeps working unchanged: a filter, an ordering, a second join, or a pagination added to the statement sees the columns of the relation under the names the marker declares.

| The marker declares | The statement reads |
|---|---|
| `UserTotals => "user_totals"` | the relation name used by `.with(...)` and by the join |
| `TotalsOwnerId => "owner_id": i64` | the `"owner_id"` column, projected as `i64` |
| `TotalsTotal => "total": f64` | the `"total"` column, projected as `f64` |

Those column names are the ones the inner statement projects, which is why the projection fields of the aggregate carry them. Naming a column the aggregate does not project asks the statement for a column that does not exist, so the two halves are written together.

### Derived tables and `LATERAL`

A finished statement also participates in a query without being given a name of its own, and — on dialects that support it — can be evaluated once per row of the relation it is joined with:

| What you need | What you write |
|---|---|
| a named statement the query reads (`WITH`) | `.as_cte::<Marker>()` and `.with(...)` |
| a statement used as a relation in the join list | `.as_table::<Marker>()` and `.inner_join_subquery(...)` / `.left_join_subquery(...)` |
| a statement evaluated for every row it is joined with | `.as_lateral::<Marker>()` and `.cross_join_lateral(...)` / `.left_join_lateral(...)` |

A derived table renders inline as `(SELECT ...) AS "name"`, so the joined relation behaves like a stored one while its rows come from the statement; a left-joined derived table lifts its columns to `Option<T>` exactly like a left-joined stored table. `LATERAL` is a dialect capability: PostgreSQL supports it, SQLite does not, so the methods do not exist there at compile time. The join side of both is covered in [Joins](joins.md), and the correlated reference they are written with in [Subqueries](subqueries.md).
A derived table renders inline as `(SELECT ...) AS "name"`, so the joined relation behaves like a stored one while its rows come from the statement; a left-joined derived table lifts its columns to `Option<T>` exactly like a left-joined stored table. `LATERAL` is a dialect capability: PostgreSQL supports it, SQLite does not, so the methods do not exist there at compile time.
## Dialect capability gates

| Feature | PostgreSQL | SQLite |
|---|---|---|
| `WITH name AS (SELECT ...)` | every declared version | every declared version |
| several named relations in one `WITH` | every declared version | every declared version |
| reading a named relation in `FROM` / `JOIN` | every declared version | every declared version |
| derived table `(SELECT ...) AS "name"` | every declared version | every declared version |
| `LATERAL`, `LEFT JOIN LATERAL` | every declared version | never |
| `WITH RECURSIVE` | not offered | not offered |

`LATERAL` is gated by a dialect capability, so on SQLite the methods that take a lateral relation do not exist and the compiler refuses the call instead of the engine refusing the statement. A recursive `WITH` is not part of the API, receives no builder method, and is listed in the capability matrix of the [Appendices](../appendix/matrix.md) as unavailable.
`LATERAL` is gated by a dialect capability, so on SQLite the methods that take a lateral relation do not exist and the compiler refuses the call instead of the engine refusing the statement. A recursive `WITH` is not part of the API, receives no builder method, and is unavailable.
## Compile-time and typestate guards

- Rows are published only by a statement that projects them: `as_cte`, `as_table`, and `as_lateral` do not exist on a query without a projection, so a relation can never be named after a statement that would render `SELECT  FROM ...`.
- A named relation joins the scope through `.with(...)`. Projecting its columns before that is refused at the call site, and the diagnostic note points at the remedy — introduce the column's table with `from` or a join before projecting it.
- Two relations under one name are refused while the statement is built, before any SQL reaches the engine; alias one of them, as [Aliases](aliasing.md) describes.
- The engine recorded by the named statement has to match the engine of the query that reads it, so a relation built for one dialect does not compile into a statement of another.
Two relations under one name are refused while the statement is built, before any SQL reaches the engine; alias one of them so each occurrence carries a name of its own.
```rust,compile_fail
// Rejected: `user_totals` is named by the marker, but this statement records no
// projection, so there is no statement to publish as a relation.
let totals = select_from(User).as_cte::<UserTotals>();
```

The accepted form projects the statement first — `select_from(Order).group_by(OrderUserId).select(cols! { ... }).as_cte::<UserTotals>()`.

```rust,compile_fail
// Rejected: the query never reads `user_totals`, so the column is out of scope.
let rows = select_from(User).select(cols!(UserId, TotalsTotal));
```

The accepted form introduces the relation with `.with(...)` — and then either joins it or reads its columns directly.
