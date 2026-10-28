# DML: `INSERT`, `UPDATE`, `DELETE`

The three mutations share one value model: an [`Assignment`] binds a value to a single column, and an [`AssignmentSet`] targets a single table with a tuple of assignments. The builders differ only in how they render the set.

## Assignments

```rust
pub enum AssignValue {
    /// A concrete value bound to a query parameter.
    Value(SqlVal),
    /// The SQL `DEFAULT` keyword, delegating to the column's default definition.
    Default,
    /// A scalar expression rendered into the statement instead of a parameter.
    Expr(ExprKind),
}
```

Every column marker offers three constructors:

| Constructor | Renders |
|---|---|
| `UserId::set(1i64)` | a bound parameter, checked against `Column::RustType` through `SqlCompatible` |
| `UserId::default_value()` | the `DEFAULT` keyword — for generated keys |
| `UserId::set_expr(expr)` | the expression inline — the form that makes `EXCLUDED."column"` reachable in an upsert |

A tuple of assignments is an `AssignmentSet<T>` only when every element targets the same table `T`, so mixing columns of unrelated tables does not compile.

## `INSERT`

```rust
use rurqe::postgres::prelude::*;

let built = insert_into(User)
    .values((UserId::set(1i64), UserName::set("root")))
    .build()
    .expect("valid query");
```

```text
INSERT INTO "users" ("id", "name") VALUES ($1, $2)
```

- `.values(...)` records one row; `.values_many(...)` records many.
- `.columns(...).from_select(query)` inserts the rows of a `SELECT` without a client round trip, and `RETURNING` rides along.
- `.returning(...)` projects the affected rows and is gated by `SupportsReturning`.
- The typestates: `build` stays unreachable until a row source is recorded (`InsertSource`), and a row source known to be empty yields a `Result` instead of the query (`EmptyRows`).

An upsert appends an `ON CONFLICT` clause:

```rust
let upsert = insert_into(User)
    .values((UserId::set(1i64), UserName::set("root")))
    .on_conflict(UserId)
    .do_update((UserName.set_expr(excluded(UserName)),))
    .build()
    .expect("valid upsert");
```

```text
INSERT INTO "users" ("id", "name") VALUES ($1, $2)
ON CONFLICT ("id") DO UPDATE SET "name" = EXCLUDED."name"
```

- `.on_conflict(target)` and `.on_conflict_do_nothing(target)` require `SupportsOnConflict`.
- `.target_where(pred)` restricts the conflict target to a partial unique index predicate.
- `.update_where(pred)` restricts the `DO UPDATE` to the rows satisfying a predicate.
- `excluded(Col)` references the proposed row through the `EXCLUDED` pseudo-relation.

## `UPDATE`

```rust
use rurqe::postgres::prelude::*;

let built = update(User)
    .set(UserName::set("root"))
    .where_(UserId.eq(1i64))
    .build()
    .expect("valid query");
```

```text
UPDATE "users" SET "name" = $1 WHERE ("id" = $2)
```

- Two typestates guard the build: `Unfiltered` → `Filtered` through `.where_(...)` or `.allow_all()`, and `NoAssignments` → `HasAssignments` through `.set(...)`.
- A recorded assignment set that collapses to zero assignments is rejected as `BuildError::EmptyUpdateSet` before any SQL is rendered, so the statement never carries a `SET` clause with nothing to assign.
- `.from_table(other)` renders `UPDATE ... FROM`, gated by `SupportsUpdateFrom`.
- `.returning(...)` is gated by `SupportsReturning`.

## `DELETE`

```rust
use rurqe::postgres::prelude::*;

let built = delete_from(User)
    .where_(UserId.eq(1i64))
    .build()
    .expect("valid query");
```

```text
DELETE FROM "users" WHERE ("id" = $1)
```

- `.using(...)` / `.using_table(...)` render `DELETE ... USING`, gated by `SupportsDeleteUsing`.
- `.returning(...)` is gated by `SupportsReturning`.

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
> ```n

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
