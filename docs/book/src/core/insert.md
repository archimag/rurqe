# DML: `INSERT`

An `INSERT` writes rows into exactly one table. The chain names the target table, then the row source — literal rows or a `SELECT` — and optionally the policy for a row that collides with a unique constraint and the projection of the written rows. Every fragment on this page is included from `examples/crud.rs`, so the page cannot drift from the code it documents.

## Clause anatomy

```sql
INSERT INTO table (column, ...)
    { VALUES (value, ...) [, (value, ...)]
    | SELECT ... }
    [ ON CONFLICT [ (column, ...) [ WHERE condition ] ]
        { DO NOTHING
        | DO UPDATE SET column = value, ... [ WHERE condition ] } ]
    [ RETURNING column, ... ]
```

| Clause | Purpose | Optional | What it changes |
|---|---|---|---|
| `INSERT INTO table (columns)` | the table written and the columns filled | no | fixes the table and the column list every row has to match |
| `VALUES (...)` | rows supplied by the client | one row source is required | records one literal row, or many rows in one statement |
| `SELECT ...` | rows copied from another relation | the alternative to `VALUES` | reads and writes in a single round trip |
| `ON CONFLICT` | the policy for a colliding row | yes | turns the statement into an upsert |
| `RETURNING` | the written rows projected back | yes | changes the result type of the statement |

The shortest executable statement records one row — `insert_into(User).values((UserId::set(1i64), UserName::set("root")))` — and the walkthrough below starts from exactly that chain.

## SQL ↔ DSL mapping

| SQL clause | Builder method | Effect on the statement |
|---|---|---|
| `INSERT INTO "users"` | `insert_into(User)` | names the table; the statement still has no row source, so it is not executable and cannot be built |
| `("id", "name", "email")` | the column list of `.values(...)`, `.values_many(...)`, or `.from_select(...)` | the written columns; each value is checked against its column type |
| `VALUES (...)` | `.values(row)` | records one row and makes the statement executable |
| `VALUES (...), (...)` | `.values_many([row, row])` | records many rows; an empty list yields an error instead of a query |
| `SELECT ...` | `.from_select((columns...), query)` | records the row source from another relation and checks the source projection against the column list |
| `ON CONFLICT ... DO NOTHING` | `.on_conflict_do_nothing()` | appends the policy without naming a constraint |
| `ON CONFLICT (columns) DO ...` | `.on_conflict(columns)` | names the conflict target; the builder offers `.do_nothing()` and `.do_update(assignments)` |
| `ON CONFLICT (columns) WHERE condition` | `.target_where(pred)` | narrows the target to a partial unique index |
| `DO UPDATE SET ... WHERE condition` | `.update_where(pred)` | restricts the update to the rows the predicate matches |
| `RETURNING ...` | `.returning(...)` | projects the written rows instead of a row count |
| — | `.build()` | renders the statement, which is only reachable once a row source is recorded |

## Clause-by-clause walkthrough

### `VALUES`: one row

```rust,no_run
# extern crate rurqe;
# use rurqe::postgres::prelude::*;
# table! { User => "users" { UserId => "id": i64, UserName => "name": String, UserEmail => "email": String, } }
# async fn example() -> Result<()> {
#     let pool: PgAsyncPool = todo!();
{{#rustdoc_include ../../../../examples/crud.rs:insert}}
#     Ok(())
# }
# fn main() {}
```

```text
INSERT INTO "users" ("id", "name", "email") VALUES ($1, $2, $3) RETURNING "id"
```

`.values(...)` takes a tuple of assignments that all target the table named by `insert_into`. The tuple is the column list: the columns are rendered in the order the assignments were written, and the parameters follow that same order. The statement returns the written `"id"` instead of a row count, because `.returning(...)` recorded a projection.

### Binding a value to a column

| Constructor | Renders |
|---|---|
| `UserId::set(1i64)` | a bound parameter, checked against the column type |
| `UserId::default_value()` | the `DEFAULT` keyword, for a generated key |
| `UserId::set_expr(expr)` | an expression written inline — the form that makes `EXCLUDED."id"` reachable in an upsert |

A tuple of assignments is accepted only when every element targets the same table, so a statement cannot mix columns of unrelated relations.

### `VALUES`: many rows

```rust,no_run
# extern crate rurqe;
# use rurqe::postgres::prelude::*;
# table! { User => "users" { UserId => "id": i64, UserName => "name": String, UserEmail => "email": String, } }
# async fn example() -> Result<()> {
#     let pool: PgAsyncPool = todo!();
{{#rustdoc_include ../../../../examples/crud.rs:values_many}}
#     Ok(())
# }
# fn main() {}
```

```text
INSERT INTO "users" ("id", "name", "email") VALUES ($1, $2, $3), ($4, $5, $6)
```

One statement carries the whole batch, so the parameters are numbered across the rows. `.values_many(...)` yields a `Result`: an empty list is refused instead of rendering a `VALUES` list with no row in it, which no engine accepts.

### `SELECT`: rows from another relation

```rust,no_run
# extern crate rurqe;
# use rurqe::postgres::prelude::*;
# table! { User => "users" { UserId => "id": i64, UserName => "name": String, UserEmail => "email": String, } }
# table! { ArchivedUser => "archived_users" { ArchivedUserId => "id": i64, ArchivedUserName => "name": String, ArchivedUserEmail => "email": String, } }
# async fn example() -> Result<()> {
#     let pool: PgAsyncPool = todo!();
{{#rustdoc_include ../../../../examples/crud.rs:from_select}}
#     Ok(())
# }
# fn main() {}
```

```text
INSERT INTO "users" ("id", "name", "email") SELECT "id", "name", "email" FROM "archived_users" WHERE ("id" > $1)
```

The source projection is a tuple that has to match the target column list in arity and type, and the nested statement keeps its own `WHERE` and its own parameter numbering. Nothing travels to the client: the engine reads one relation and writes the other. `RETURNING` rides along exactly as it does with literal rows.

### `ON CONFLICT`: the upsert

```rust,no_run
# extern crate rurqe;
# use rurqe::postgres::prelude::*;
# table! { User => "users" { UserId => "id": i64, UserName => "name": String, } }
# async fn example() -> Result<()> {
#     let pool: PgAsyncPool = todo!();
{{#rustdoc_include ../../../../examples/crud.rs:upsert}}
#     Ok(())
# }
# fn main() {}
```

```text
INSERT INTO "users" ("id", "name") VALUES ($1, $2) ON CONFLICT DO NOTHING
```

- `.on_conflict_do_nothing()` appends the policy without naming a constraint.
- `.on_conflict(columns)` names the conflict target; the returned builder offers `.do_nothing()` and `.do_update(assignments)`.
- `.target_where(pred)` narrows the target to a partial unique index, rendering `ON CONFLICT (...) WHERE ...`.
- `.update_where(pred)` adds the `WHERE` of `DO UPDATE`, so only the rows the predicate matches are overwritten.
- `excluded(Col)` references the rejected row's proposed value as a typed column: it renders `EXCLUDED."name"` and is checked against the column it wraps, so an assignment can never read the wrong kind of value from the rejected row.

Both conflict predicates render natively. There is no `CASE WHEN` emulation and no client round trip behind the clause.

### `RETURNING`

`returning(...)` accepts the same named lists as a projection — single columns, tuples, or whole record selections. The clause addresses the written row alone, which is why `.returning(cols!(UserId))` above yields the generated key rather than a row count, and why a column of another relation is not part of what it can project.

## Dialect capability gates

| Clause or feature | PostgreSQL | SQLite |
|---|---|---|
| `INSERT INTO ... VALUES` | every declared version | every declared version |
| `INSERT INTO ... SELECT` | every declared version | every declared version |
| `RETURNING` | every declared version | from 3.35 |
| `ON CONFLICT ... DO NOTHING` | every declared version | from 3.24 |
| `ON CONFLICT (...) DO UPDATE`, `EXCLUDED` | every declared version | from 3.24 |

Below the stated threshold the clause does not exist on the target, so the method cannot even be named there: the compiler refuses the call and the diagnostic names the unsupported clause instead of a trait resolution cascade.

## Compile-time and typestate guards

- An `INSERT` without a row source is not executable, and not even buildable: `.build()` and the fetch helpers appear only after `.values(...)`, `.values_many(...)`, or `.from_select(...)` records one.
- A row source known to be empty yields an error instead of a query, so the statement can never render `VALUES ()`.
- A row that assigns nothing, or a row that disagrees with the first row on the number of values or on the column at a position, is rejected while the statement is assembled — before any SQL reaches the engine.

```rust,compile_fail
# extern crate rurqe;
# use rurqe::postgres::prelude::*;
# table! { User => "users" { UserId => "id": i64, UserName => "name": String, } }
# fn main() {
// Rejected: the statement names a table but records no row to insert.
let written = insert_into(User).build();
# let _ = written;
# }
```

The accepted form records the rows first — `.values((UserId::set(1i64), UserName::set("root")))` — and only then builds or executes the statement.
