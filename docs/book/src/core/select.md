# Projections and `SELECT`

A projection decides which attributes travel to the client and in which order. In RURQE it is written once at the call site, and it fixes the row type of the query: decoding, `RETURNING`, a nested collection, and a derived table all read that same shape.

## Clause anatomy

```sql
SELECT [DISTINCT [ON (expression, ...)]]
    projection [, ...]
    [ FROM relation [, ...] [ join ... ] ]
    [ WHERE condition ]
    [ GROUP BY expression, ... ]
    [ HAVING condition ]
    [ ORDER BY expression [ASC | DESC] [NULLS FIRST | NULLS LAST], ... ]
    [ LIMIT n ] [ OFFSET n ]
    [ FOR UPDATE | FOR SHARE [ NOWAIT | SKIP LOCKED ] ]
```

| Clause | Purpose | Optional | What it changes |
|---|---|---|---|
| `SELECT` | the projected attributes | no | fixes the row type of the query and the output column names |
| `DISTINCT` / `DISTINCT ON (...)` | removes duplicate rows, or keeps one row per group | yes | removes the row-locking clauses from the statement |
| `FROM` / `JOIN` | the relations read | no for a stored relation | adds a relation to the scope; an outer join lifts its columns to `Option<T>` |
| `WHERE` | the row filter | yes | restricts which rows are read, never the row type |
| `GROUP BY` / `HAVING` | the grouping of the rows and the filter over the groups | yes | restricts the projection to grouped columns, aggregates, constants, and windows |
| `ORDER BY`, `LIMIT`, `OFFSET` | the order of the result and its window | yes | changes the ordering and the number of rows, never the row type |
| `FOR UPDATE` / `FOR SHARE` | claims the read rows for the current transaction | yes | makes the statement a locking statement of its own |

The shortest statement projects and reads:

```rust,no_run
# extern crate rurqe;
# use rurqe::postgres::prelude::*;
# table! { User => "users" { UserId => "id": i64, UserName => "name": String, } }
# fn main() -> Result<(), Box<dyn std::error::Error>> {
# let mut conn: PgConnection = todo!();
let rows = select_from(User)
    .select(cols!(UserId, UserName))
    .fetch_all(&mut conn)?;
# let _ = rows;
# Ok(())
# }
```

## SQL ↔ DSL mapping

| SQL clause | Builder method | Effect on the statement |
|---|---|---|
| `SELECT ...` | `select!(...)` at the head of a chain, `.select(cols!(...))` inside one | records the projection and makes the statement renderable |
| the output column names | the field name of every item: the column name, `col as alias`, or `alias: expr` | the names a CTE, a derived table, and a compound ordering address |
| `DISTINCT` | `.distinct()` | drops duplicates; `.for_update()` and `.for_share()` leave the statement |
| `DISTINCT ON (...)` | `.distinct_on(col)` — PostgreSQL | the same lock rule; the ordering has to start with the clause |
| `FROM` | `select_from(table)` / `.from(table)` | introduces the relation and the declared types of its columns |
| `JOIN` | `.inner_join(...)`, `.left_join(...)`, `.cross_join(...)`, `.full_outer_join(...)`, `.inner_join_subquery(...)`, `.left_join_subquery(...)`, `.cross_join_lateral(...)`, `.left_join_lateral(...)` | adds a relation to the scope; an outer join lifts its columns to `Option<T>` |
| `WHERE` | `.where_(predicate)`, `.where_opt(option)`, `.when(condition, closure)`, `.if_some(option, closure)` | restricts the rows and keeps the row type |
| `GROUP BY` | `.group_by(columns)` | restricts the projection to grouped columns, aggregates, constants, and windows |
| `HAVING` | `.having(predicate)`, `.having_col(...)` | filters the groups the aggregate produced |
| `ORDER BY` | `.order_by_asc(...)`, `.order_by_desc(...)`, `.order_by_nulls(...)`, `.order_by_expr(...)` | orders the result |
| `LIMIT` / `OFFSET` | `.limit(n)` / `.offset(n)` | windows the result |
| `UNION`, `INTERSECT`, `EXCEPT` | `.union(query)`, `.union_all(query)`, `.intersect(query)`, `.except(query)` | combines statements and closes the projection list |
| the `ORDER BY` of a combined statement | `.order_by_output_asc(...)`, `.order_by_index(n)` | orders the combined rows; no further operand can follow |
| `FOR UPDATE` / `FOR SHARE` | `.for_update()` / `.for_share()`, then `.nowait()` / `.skip_locked()` — PostgreSQL | claims the rows; the statement can no longer be nested or combined |
| — | `.build()` | renders the statement; reachable only once a projection is recorded |

## Clause-by-clause walkthrough

The clauses below follow the order of the anatomy. The filter, the joins, the grouping, the ordering, and the locking each have a chapter of their own — [Predicates and `WHERE`](filtering.md), [Joins](joins.md), [`GROUP BY` and `HAVING`](group_by.md), [Ordering and pagination](ordering.md), [Row locking](../dialects/postgres/locking.md) — so this walkthrough goes through them briefly and stays on the projection, which is what the row type is made of.

### The projection: `select!` and `cols!`

```rust,no_run
# extern crate rurqe;
# use rurqe::postgres::prelude::*;
# table! { User => "users" { UserId => "id": i64, UserName => "name": String, UserActive => "active": bool, } }
# fn main() -> Result<(), Box<dyn std::error::Error>> {
# let mut conn: PgConnection = todo!();
let active = select!(UserId, UserName as contact_email)
    .from(User)
    .where_(UserActive.eq(true))
    .order_by_asc(UserId)
    .fetch_all(&mut conn)?;

for row in active {
    println!("#{} <{}>", row.user_id, row.contact_email);
}
# Ok(())
# }
```

The field names come from the markers: `UserId` becomes `user_id`, and `UserName as contact_email` becomes `contact_email`. A marker named after a Rust keyword is written with a raw identifier (`Type` is `r#type`) and still carries the bare keyword as its output column.

`cols!` is the same macro under a second name — the spelling that reads better when the list is handed to `.select(...)`:

```rust,no_run
# extern crate rurqe;
# use rurqe::postgres::prelude::*;
# table! { User => "users" { UserId => "id": i64, UserName => "name": String, } }
# fn main() -> Result<(), Box<dyn std::error::Error>> {
# let mut conn: PgConnection = todo!();
let rows = select_from(User)
    .select(cols!(UserId, UserName as contact_email))
    .fetch_all(&mut conn)?;
# let _ = rows;
# Ok(())
# }
```

Both forms produce the same record and the same SQL, and every element is rendered under its field name. That naming is what keeps the columns reachable when the query becomes a derived table or a CTE.

A positional tuple projection still exists — for low-level code, and for a row that feeds a typed collection of pairs directly (see [Runtime: overview](../runtime/overview.md)). This chapter does not use it, because a named projection survives a change in the column order.

### Computed projections

Anything that renders as a value can be a field — write the field name yourself:

```rust,no_run
# extern crate rurqe;
# use rurqe::postgres::prelude::*;
# table! { Order => "orders" { OrderId => "id": i64, OrderTotal => "total": f64, } }
# fn main() -> Result<(), Box<dyn std::error::Error>> {
# let mut conn: PgConnection = todo!();
let rows = select_from(Order)
    .select(cols!(
        order_id: OrderId,
        total: OrderTotal,
        billed: coalesce(OrderTotal, 0.0f64),
    ))
    .fetch_all(&mut conn)?;
# let _ = rows;
# Ok(())
# }
```

`billed` has the type of the expression (`f64`), not the type of a stored column.

### One name per field

Duplicate field names are refused while the macro expands, and the message carries the remedy:

```text
projection field `id` is produced by more than one projection; rename one of them with `as <alias>` or `alias: <expr>`
```

### `dto!`: a named row shape

`select!` is anonymous — exactly right inside a query, useless in a signature. `dto!` declares the shape once and reads the field types from the schema instead of repeating them:

```rust
# extern crate rurqe;
# use rurqe::prelude::*;
# table! { User => "users" { UserId => "id": i64, UserName => "name": String, UserActive => "active": bool, } }
dto! {
    /// A user row crossing a repository boundary.
    pub struct UserSummary {
        UserId,
        UserName as contact_email,
        UserActive,
    }
}
# fn main() {}
```

The declaration implements `FromRow`, so a row decodes by field name:

```rust
# extern crate rurqe;
# use rurqe::prelude::*;
# table! { User => "users" { UserId => "id": i64, UserName => "name": String, UserActive => "active": bool, } }
# dto! { pub struct UserSummary { UserId, UserName as contact_email, UserActive, } }
# fn main() {
let row = UserSummary::from_row(&[
    SqlVal::I64(7),
    SqlVal::Text("root".to_owned()),
    SqlVal::Bool(true),
])
.expect("row decodes");

assert_eq!(row.user_id, 7);
assert_eq!(row.contact_email, "root");
# }
```

It also implements the nested-row contracts, so `Vec<UserSummary>` decodes straight out of a collection document (see [Nested results](../features/nested.md)).

A DTO is a row shape, not a projection: its fields must be column markers, and a computed field is refused during expansion with a message that points at `select!`. The statement projects with `select!`; the DTO names what the row is.

### `DISTINCT` and `DISTINCT ON`

`.distinct()` keeps one row per distinct combination of the projected attributes; `.distinct_on(col)` — PostgreSQL alone — keeps one row per group and needs the ordering to start with the same expression. Both remove the row-locking clauses from the statement, because the lock claims rows the deduplication then discards.

```rust,no_run
# extern crate rurqe;
# use rurqe::postgres::prelude::*;
# table! { User => "users" { UserName => "name": String, } }
# fn main() -> Result<(), Box<dyn std::error::Error>> {
# let mut conn: PgConnection = todo!();
let unique_names = select_from(User)
    .select(cols!(UserName))
    .distinct()
    .order_by_asc(UserName)
    .fetch_all(&mut conn)?;
# let _ = unique_names;
# Ok(())
# }
```

### `FROM` and joins

`select_from(table)` and `.from(table)` introduce the relation a projection is read from, and every join adds one more relation to that scope. The joins are the subject of [Joins](joins.md) and [Aliases](aliasing.md); the part a projection feels is the nullability of an outer join, described below.

### `WHERE`

`.where_(predicate)` restricts the rows; `.where_opt(option)`, `.when(condition, closure)`, and `.if_some(option, closure)` add the same predicate conditionally, without leaving the chain or changing the row type. The predicate model is described in [Predicates and `WHERE`](filtering.md).

### `GROUP BY` and `HAVING`

```rust,no_run
# extern crate rurqe;
# use rurqe::postgres::prelude::*;
# table! { Order => "orders" { OrderUserId => "user_id": i64, OrderTotal => "total": f64, } }
# fn main() -> Result<(), Box<dyn std::error::Error>> {
# let mut conn: PgConnection = todo!();
let rows = select_from(Order)
    .group_by(OrderUserId)
    .select(cols!(
        user_id: OrderUserId,
        orders: count_all(),
        spend: sum(OrderTotal),
    ))
    .fetch_all(&mut conn)?;
# let _ = rows;
# Ok(())
# }
```

After `.group_by(...)` a field is accepted only when it is a grouped column, an aggregate, a constant, or a window over those. A stored column that is neither is refused at the call site as a column the grouping dropped. Aggregates inside scalar functions keep that property: `coalesce(sum(OrderTotal), 0.0f64)` is still an aggregate. [`GROUP BY` and `HAVING`](group_by.md) covers the query type the clause produces.

### `ORDER BY`, `LIMIT`, `OFFSET`

The `.order_by_asc(...)`, `.order_by_desc(...)`, `.order_by_nulls(...)`, and `.order_by_expr(...)` families record the ordering of the produced rows, and `.limit(n)` / `.offset(n)` window them. None of them changes the row type. [Ordering and pagination](ordering.md) describes the criteria, the null placement, and the keyset forms.

### `FOR UPDATE` and `FOR SHARE`

`.for_update()` and `.for_share()`, with `.nowait()` or `.skip_locked()`, claim the read rows for the current transaction. The capability is PostgreSQL-only, and a locking statement is a statement of its own: it cannot be nested inside another statement or become an operand of a set operation. See [Row locking](../dialects/postgres/locking.md).

### Nullability comes from the join

Write the left join and keep the same projection:

```rust,no_run
# extern crate rurqe;
# use rurqe::postgres::prelude::*;
# table! { User => "users" { UserId => "id": i64, } }
# table! { Order => "orders" { OrderUserId => "user_id": i64, OrderTotal => "total": f64, } }
# fn main() -> Result<(), Box<dyn std::error::Error>> {
# let mut conn: PgConnection = todo!();
let report = select_from(User)
    .left_join(Order, OrderUserId.eq(UserId))
    .select(cols!(user_id: UserId, order_total: OrderTotal))
    .fetch_all(&mut conn)?;

for row in report {
    match row.order_total {
        Some(total) => println!("#{}: {total}", row.user_id),
        None => println!("#{}: no orders", row.user_id),
    }
}
# Ok(())
# }
```

```text
SELECT "users"."id" AS "user_id", "orders"."total" AS "order_total"
FROM "users" LEFT JOIN "orders" ON ("orders"."user_id" = "users"."id")
```

`row.order_total` is `Option<f64>`, and nothing in the projection had to mention it — the type follows from the join, so reading the field as `f64` does not compile. The same lifting applies to `FULL OUTER JOIN` (both sides) and to a left-joined derived table; see [Nullability](../theory/nullability.md).

### `RETURNING`

The mutation side of the same story: `RETURNING` accepts the same named lists as a projection.

```rust,no_run
# extern crate rurqe;
# use rurqe::postgres::prelude::*;
# table! { User => "users" { UserId => "id": i64, UserName => "name": String, } }
# fn main() {
let renamed = update(User)
    .set(UserName::set("root"))
    .where_(UserId.eq(1i64))
    .returning(cols!(UserName));
# let _ = renamed;
# }
```

The clause writes back into the mutated row, so it accepts only a projection of that row: a column of another table names a relation the clause cannot address, and a computed value is a value the clause cannot write back. Both are refused at the call site, before any SQL is rendered.

### The nullable shape, explicitly

Sometimes the nullable form is needed without an outer join — to align a projection with an already computed result. `query.nullable::<OrderTotal>()` is the same column with `Option<f64>` as its value type; a second layer never appears, because `Option<Option<T>>` collapses into `Option<T>`.

## Dialect capability gates

| Feature | PostgreSQL | SQLite |
|---|---|---|
| `SELECT` with a projection | every declared version | every declared version |
| `DISTINCT` | every declared version | every declared version |
| `DISTINCT ON (...)` | every declared version | never |
| window functions (`OVER (...)`) | every declared version | from 3.25 |
| aggregate `FILTER (WHERE ...)` | every declared version | from 3.30 |
| `NULLS FIRST` / `NULLS LAST` | every declared version | from 3.30 |
| `FULL OUTER JOIN` | every declared version | never |
| `LATERAL` joins | every declared version | never |
| `FOR UPDATE` / `FOR SHARE`, `NOWAIT`, `SKIP LOCKED` | every declared version | never |
| `GREATEST` / `LEAST` in a projection | every declared version | never |
| `IS [NOT] DISTINCT FROM` in a predicate | every declared version | never |
| nested collections (`nest_many` / `nest_one`) | `jsonb_agg` | from 3.38 |

Below the stated threshold the feature does not exist on the target, so the method cannot even be named there: the compiler refuses the call and the diagnostic names the unsupported clause instead of a trait resolution cascade. The full matrix is in the [Appendices](../appendix/matrix.md).

## Compile-time and typestate guards

- A query without a projection does not render: `.build()` and the fetch helpers are not available on it.
- A projection is recorded once. After `UNION`, `INTERSECT`, or `EXCEPT` the output columns are the ones the first list named, so `.select(...)` is unreachable on the combined statement; ordering the combined rows goes through `.order_by_output_asc(...)` or `.order_by_index(...)`.
- Every projected item must belong to a relation the statement introduces. A column of a table outside the scope is refused at the call site, and the diagnostic note points at the remedy — introduce the column's table with `from` or a join before projecting it.
- A hand-written selection that records nothing is rejected while the statement is built, instead of rendering `SELECT  FROM "users"`.

```rust,compile_fail
# extern crate rurqe;
# use rurqe::postgres::prelude::*;
# table! { User => "users" { UserId => "id": i64, } }
# fn main() {
// Rejected: nothing is projected, so there is no statement to build.
let statement = select_from(User).build();
# let _ = statement;
# }
```

The accepted form projects first — `select_from(User).select(cols!(UserId)).build()`.

```rust,compile_fail
# extern crate rurqe;
# use rurqe::postgres::prelude::*;
# table! { User => "users" { UserId => "id": i64, } }
# table! { Order => "orders" { OrderUserId => "user_id": i64, OrderTotal => "total": f64, } }
# fn main() {
// Rejected: `OrderTotal` belongs to `orders`, a relation this query never reads.
let rows = select_from(User).select(cols!(UserId, OrderTotal));
# let _ = rows;
# }
```

The accepted form introduces the relation before projecting its column: `select_from(User).inner_join(Order, OrderUserId.eq(UserId)).select(cols!(UserId, OrderTotal))` — and there the field arrives as `Option<f64>`, because the join is an outer one only when it is written as `.left_join(...)`.

## Related chapters

- [Predicates and `WHERE`](filtering.md)
- [Ordering and pagination](ordering.md)
- [`GROUP BY` and `HAVING`](group_by.md)
- [Aliases](aliasing.md)
- [DML: `INSERT`](insert.md) — the mutation side that `RETURNING` writes back
