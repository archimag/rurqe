# DML: `INSERT`, `UPDATE`, `DELETE`

The three mutations share one value model: an `Assignment` binds a value to a single column, and an `AssignmentSet` targets a single table with a tuple of assignments. The builders differ only in how they render the set. Every statement below is included from `examples/crud.rs`.

## Assignments

```rust
# extern crate rurqe;
# use rurqe::core::{ExprKind, SqlVal};
pub enum AssignValue {
    /// A concrete value bound to a query parameter.
    Value(SqlVal),
    /// The SQL `DEFAULT` keyword, delegating to the column's default definition.
    Default,
    /// A scalar expression rendered into the statement instead of a parameter.
    Expr(ExprKind),
}
# fn main() {}
```

Every column marker offers three constructors:

| Constructor | Renders |
|---|---|
| `UserId::set(1i64)` | a bound parameter, checked against `Column::RustType` through `SqlCompatible` |
| `UserId::default_value()` | the `DEFAULT` keyword — for generated keys |
| `UserId::set_expr(expr)` | the expression inline — the form that makes `EXCLUDED."column"` reachable in an upsert |

A tuple of assignments is an `AssignmentSet<T>` only when every element targets the same table `T`, so mixing columns of unrelated tables does not compile.

## `INSERT`

```rust,no_run
# extern crate rurqe;
# use rurqe::postgres::prelude::*;
# table! { User => "users" { UserId => "id": i64, UserName => "name": String, UserEmail => "email": String, } }
# fn main() {
{{#rustdoc_include ../../../../examples/crud.rs:insert}}
# }
```

```text
INSERT INTO "users" ("id", "name", "email") VALUES ($1, $2, $3) RETURNING "id"
```

- `.values(...)` records one row; `.values_many(...)` records many.
- `.columns(...).from_select(query)` inserts the rows of a `SELECT` without a client round trip, and `RETURNING` rides along.
- `.returning(...)` projects the affected rows and is gated by `SupportsReturning`.
- The typestates: `build` stays unreachable until a row source is recorded (`InsertSource`), and a row source known to be empty yields a `Result` instead of the query (`EmptyRows`).

Inserting many rows records a single multi-row `VALUES` list:

```rust,no_run
# extern crate rurqe;
# use rurqe::postgres::prelude::*;
# table! { User => "users" { UserId => "id": i64, UserName => "name": String, UserEmail => "email": String, } }
# fn main() {
{{#rustdoc_include ../../../../examples/crud.rs:values_many}}
# }
```

```text
INSERT INTO "users" ("id", "name", "email") VALUES ($1, $2, $3), ($4, $5, $6)
```

Copying rows from another relation goes through `.from_select(columns, query)`, so the statement reads and writes in a single round trip. The source projection is a tuple matching the target column list exactly:

```rust,no_run
# extern crate rurqe;
# use rurqe::postgres::prelude::*;
# table! { User => "users" { UserId => "id": i64, UserName => "name": String, UserEmail => "email": String, } }
# table! { ArchivedUser => "archived_users" { ArchivedUserId => "id": i64, ArchivedUserName => "name": String, ArchivedUserEmail => "email": String, } }
# fn main() {
{{#rustdoc_include ../../../../examples/crud.rs:from_select}}
# }
```

```text
INSERT INTO "users" ("id", "name", "email") SELECT "id", "name", "email" FROM "archived_users" WHERE ("id" > $1)
```

An upsert appends an `ON CONFLICT` clause:

```rust,no_run
# extern crate rurqe;
# use rurqe::postgres::prelude::*;
# table! { User => "users" { UserId => "id": i64, UserName => "name": String, } }
# fn main() {
{{#rustdoc_include ../../../../examples/crud.rs:upsert}}
# }
```

```text
INSERT INTO "users" ("id", "name") VALUES ($1, $2) ON CONFLICT DO NOTHING
```

- `.on_conflict(target)` and `.on_conflict_do_nothing(target)` require `SupportsOnConflict`.
- `.target_where(pred)` restricts the conflict target to a partial unique index predicate.
- `.update_where(pred)` restricts the `DO UPDATE` to the rows satisfying a predicate.
- `excluded(Col)` references the proposed row through the `EXCLUDED` pseudo-relation.

## `UPDATE`

```rust,no_run
# extern crate rurqe;
# use rurqe::postgres::prelude::*;
# table! { User => "users" { UserId => "id": i64, UserName => "name": String, } }
# fn main() {
{{#rustdoc_include ../../../../examples/crud.rs:update}}
# }
```

```text
UPDATE "users" SET "name" = $1 WHERE ("id" = $2)
```

- Two typestates guard the build: `Unfiltered` → `Filtered` through `.where_(...)` or `.allow_all()`, and `NoAssignments` → `HasAssignments` through `.set(...)`.
- A recorded assignment set that collapses to zero assignments is rejected as `BuildError::EmptyUpdateSet` before any SQL is rendered, so the statement never carries a `SET` clause with nothing to assign.
- `.from_table(other)` renders `UPDATE ... FROM`, gated by `SupportsUpdateFrom`.
- `.returning(...)` is gated by `SupportsReturning`.

A `FROM` source is introduced with `.from_table(other)` before the filter that reads it, and every column of the source is in scope of `SET`, the filter, and `RETURNING`:

```rust,no_run
# extern crate rurqe;
# use rurqe::postgres::prelude::*;
# table! { User => "users" { UserId => "id": i64, UserName => "name": String, } }
# table! { Account => "accounts" { AccountId => "id": i64, } }
# fn main() {
{{#rustdoc_include ../../../../examples/crud.rs:update_from}}
# }
```

```text
UPDATE "users" SET "name" = $1 FROM "accounts" WHERE ("users"."id" = "accounts"."id")
```

## `DELETE`

```rust,no_run
# extern crate rurqe;
# use rurqe::postgres::prelude::*;
# table! { User => "users" { UserId => "id": i64, } }
# fn main() {
{{#rustdoc_include ../../../../examples/crud.rs:delete}}
# }
```

```text
DELETE FROM "users" WHERE ("id" = $1)
```

- `.using(...)` / `.using_table(...)` render `DELETE ... USING`, gated by `SupportsDeleteUsing`.
- `.returning(...)` is gated by `SupportsReturning`.

A `USING` source is introduced with `.using_table(other)` and takes part in the filter like the deleted table itself:

```rust,no_run
# extern crate rurqe;
# use rurqe::postgres::prelude::*;
# table! { User => "users" { UserId => "id": i64, } }
# table! { Account => "accounts" { AccountId => "id": i64, } }
# fn main() {
{{#rustdoc_include ../../../../examples/crud.rs:delete_using}}
# }
```

```text
DELETE FROM "users" USING "accounts" WHERE ("users"."id" = "accounts"."id")
```

## Dynamic filters

| Method | Behavior |
|---|---|
| `.where_(pred)` | records the predicate; repeated calls fold with `AND` |
| `.where_opt(Option<pred>)` | records the predicate when it is present |
| `.where_opt_or_all(Option<pred>)` | records the predicate, or confirms the full-table mutation when it is absent |
| `.where_if(...)` | a conditional form of `.where_` |
| `.allow_all()` | confirms a full-table mutation explicitly |

`SET` values bind before the `WHERE` predicates, following the order of appearance (see [Predicates and `WHERE`](filtering.md)).

> **Compiler Guard in Action.** The guards are diagnostics, not runtime checks:
>
> ```text
> refusing to build a full-table mutation without a filter
>     = note: restrict the mutation with `.where_(...)`, or call `.allow_all()` to explicitly opt into modifying every row
>
> refusing to build a mutation without column values
>     = note: record at least one value with `.set(...)`, `.values(...)`, or `.from_select(...)` before building the statement
> ```

## Where this lives in the code

| Module | Contents |
|---|---|
| `rurqe-core/src/dml.rs` | `AssignValue`, `Assignment`, `AssignmentSet`, the typestates and their diagnostics |
| `rurqe-core/src/dml/insert.rs` | `InsertQuery`, `values`, `values_many`, `from_select`, `on_conflict` |
| `rurqe-core/src/dml/update.rs` | `UpdateQuery` |
| `rurqe-core/src/dml/delete.rs` | `DeleteQuery` |
| `rurqe-core/src/dml/conflict.rs` | `OnConflict` and the `ON CONFLICT` clause |
| `rurqe-core/src/dml/excluded.rs` | `excluded`, `Excluded` |

## Related chapters

- [Predicates and `WHERE`](filtering.md) — the predicate model and the dynamic forms.
- [Schema: ZST markers](schema.md) — `Column::RustType` and `ColumnBelongsTo`.
