# LLM Cheatsheet / Prompt Pack

A condensed version of the book for loading into a model context. Every
statement below is derived from the workspace code; the examples match the
doc-tests of `crates/rurqe/src/lib.rs` and the doctests of the dialect crates.

## 1. Entry point and dialect

```rust,no_run
# rurqe::table! {
#     User => "users" {
#         UserId => "id": i64,
#     }
# }
// Best practice: pin the dialect with a prelude — no turbofish anywhere.
use rurqe::postgres::prelude::*; // PostgreSQL, default Pg16

let q = select_from(User).select(cols!(UserId));
# let _ = q;
```

```rust,no_run
# rurqe::table! {
#     User => "users" {
#         UserId => "id": i64,
#     }
# }
// The same query against SQLite:
use rurqe::sqlite::prelude::*; // SQLite, default Sqlite3_35

let q = select_from(User).select(cols!(UserId));
# let _ = q;
```

```rust,no_run
# rurqe::table! {
#     User => "users" {
#         UserId => "id": i64,
#     }
# }
// An explicit engine version:
use rurqe::postgres::pg16::prelude::*; // PostgreSQL 16

let q = select_from(User).select(cols!(UserId));
# let _ = q;
```

```rust,no_run
# rurqe::table! {
#     User => "users" {
#         UserId => "id": i64,
#     }
# }
// Dialect-agnostic code (library extension, core chapters) names the dialect
// explicitly; this is the only place the turbofish form appears:
use rurqe::prelude::*;

let q = select_from::<Postgres, User>().select(cols!(UserId));
# let _ = q;
```

Version modules: `pg14`, `pg15`, `pg16`, `sqlite3_0`, `sqlite3_24`,
`sqlite3_25`, `sqlite3_30`, `sqlite3_35`, `sqlite3_38`. Each re-exports the
shared prelude and adds `select_from`, `insert_into`, `update`, `delete_from`,
the `SelectFrom` trait, and the `Error` and `Result` aliases bound to the
dialect.

## 2. Schema

```rust
rurqe::table! {
    User => "users" {
        UserId => "id": i64,
        UserName => "name": String,
        IsActive => "is_active": bool,
    }
}

rurqe::table_alias! {
    U2 => "u2"
}
```

`table!` creates ZST markers for the table and its columns; `Column::Table` ties
a column to its table and is checked through `ColumnBelongsTo<T>`.

## 3. SQL ↔ DSL mapping

| SQL | DSL |
|---|---|
| `FROM users` | `.from(users)` / `.from(Users)` |
| `SELECT a, b AS x` | `.select((a, b))` / `select! { x: b }` / `.select(cols!(A, B))` |
| `WHERE a = $1` | `.where_(a.eq(v))` |
| `WHERE a IS NULL` | `.where_(a.is_null())` |
| `WHERE a LIKE $1` | `.where_(a.like("A%"))` |
| `WHERE a ILIKE $1` | `.where_(a.ilike("A%"))` (on SQLite rendered as `LOWER(x) LIKE LOWER(?)`) |
| `WHERE a BETWEEN $1 AND $2` | `.where_(a.between(lo, hi))` |
| `WHERE a IN (...)` | `.where_(a.in_list(v))` / `.not_in_list(v)` |
| `WHERE a IN (SELECT …)` | `.where_(a.in_subquery(sub))` |
| `EXISTS (…)` / `NOT EXISTS (…)` | `.where_(exists(sub))` / `.where_(not_exists(sub))` |
| `INNER JOIN u ON …` | `.inner_join(u, u.id.eq(UserId))` |
| `LEFT JOIN u ON …` | `.left_join(u, …)` — the columns of `u` become `Option<T>` |
| `FULL OUTER JOIN` | `.full_outer_join(u, …)` — both sides become `Option<T>` |
| `CROSS JOIN` | `.cross_join(u)` |
| `LATERAL` | `.cross_join_lateral(l)` / `.left_join_lateral(l)` (PostgreSQL) |
| `GROUP BY a, b` | `.group_by((a, b))` → `GroupedQuery` |
| `HAVING COUNT(*) > 1` | `.having(count_all().gt(1i64))` |
| `ORDER BY a ASC` | `.order_by_asc(a)` |
| `ORDER BY a DESC NULLS LAST` | `.order_by_desc_nulls_last(a)` |
| `LIMIT n OFFSET m` | `.limit(n).offset(m)` |
| `SELECT DISTINCT` | `.distinct()` |
| `DISTINCT ON (a)` | `.distinct_on(a)` (PostgreSQL); `ORDER BY` must start with `a` |
| `UNION [ALL]` | `.union(q)` / `.union_all(q)` |
| `INTERSECT` / `EXCEPT` | `.intersect(q)` / `.except(q)` |
| `FOR UPDATE` / `FOR SHARE` | `.for_update()` / `.for_share()` (PostgreSQL) |
| `… SKIP LOCKED` / `NOWAIT` | `.skip_locked()` / `.nowait()` |
| `WITH cte AS (…)` | `.with(cte(q))` / `cte_named("name", q)` |
| `INSERT INTO t (…) VALUES (…)` | `insert_into(t).values((A::set(v), B::set(w)))` |
| `INSERT … SELECT` | `.columns((A, B)).from_select(select_q)` |
| `UPDATE t SET a = $1 WHERE …` | `update(t).set(a.set(v)).where_(…)` |
| `DELETE FROM t WHERE …` | `delete_from(t).where_(…)` |
| `RETURNING a` | `.returning(a)` (PostgreSQL, SQLite ≥ 3.35) |
| `ON CONFLICT (a) DO NOTHING` | `.on_conflict_do_nothing(a)` |
| `ON CONFLICT (a) DO UPDATE SET …` | `.on_conflict(a).do_update((A.set(excluded(A)),))` |

## 4. Hard rules (a violation is a compile error or a `BuildError`)

1. **No duplicate relation names.** `FROM users JOIN users` is
   `BuildError::DuplicateRelation { name }`. Remedy: `table_alias!` /
   `users.as_alias("u2")`.
2. **No names outside the scope.** A column of a table that was not introduced
   through `from`/a join is rejected at the call site (`TableInScope`,
   `ExprInScope`).
3. **An outer join lifts the type.** A column from a `LEFT JOIN` is
   `Option<T>`; reading it as `T` does not compile.
4. **An `UPDATE`/`DELETE` without a filter is not built.** Either `.where_(…)`
   or an explicit `.allow_all()` is required.
5. **A `SELECT` without a projection is not built** (`ProjectionPresent`), and
   neither is an `UPDATE` without `SET` (`AssignmentsPresent`).
6. **An `INSERT` without a row source is not built** (`InsertSource`,
   `EmptyRows`).
7. **Grouping is closed.** After `.group_by(…)` only grouped columns,
   aggregates, and constants/window expressions remain projectable
   (`GroupedSelectTarget`). An aggregate inside `coalesce`, `nullif`,
   `greatest`, `least`, or `CASE` keeps its aggregate nature through
   `CombineAggr`.
8. **An aggregate cannot stand in a row clause.** `WHERE`, `GROUP BY`, the `ON`
   condition of a join, `SET`/`VALUES`, and `ON CONFLICT` predicates are
   `BuildError::AggregateInRowClause`.
9. **`DISTINCT` + `ORDER BY`:** every ordering expression must be projected
   (`BuildError::DistinctOrderNotProjected`).
10. **`DISTINCT ON` + `ORDER BY`:** the ordering starts exactly with the
    `DISTINCT ON` expressions (`BuildError::DistinctOnOrderMismatch`).
11. **`DISTINCT`/`DISTINCT ON` are incompatible with locking**
    (`DistinctWithLock`, `DistinctOnWithLock`); the fluent typestate never
    reaches such a pair.
12. **A compound statement is ordered by its own output columns or by a 1-based
    position** (`order_by_output*`, `order_by_index`), not by expressions;
    `BuildError::CompoundOrderExpression` / `CompoundOrderColumn` /
    `CompoundOrderIndex`.
13. **The operands of `UNION`/`INTERSECT`/`EXCEPT` share one arity**
    (`BuildError::SetOperandArity`).
14. **A nested statement cannot request a lock** (`BuildError::NestedLock`).
15. **An `IN` subquery matches by arity and types** (`InSubquery<Row>`).
16. **No `JOIN … USING (col)`** and no emulation of unsupported capabilities —
    only the capability gate.

## 5. Typestates that cannot be bypassed

| Rule | Guard trait |
|---|---|
| the projection is set | `ProjectionPresent` |
| the mutation is filtered | `ExecutableMutation` |
| the assignments are set | `AssignmentsPresent` |
| the row source is set | `InsertSource`, `EmptyRows` |
| only grouped/aggregated fields | `GroupedSelectTarget`, `ColumnInGroupBy` |
| a dialect capability | `SupportsReturning`, `SupportsOnConflict`, `SupportsUpdateFrom`, `SupportsDeleteUsing`, `SupportsLateralJoin`, `SupportsFullOuterJoin`, `SupportsRowLocking`, `SupportsDistinctOn`, `SupportsAggregateFilter`, `SupportsWindowFunctions`, `SupportsNullsOrdering`, `SupportsGreatest`, `SupportsJsonAggregation`, `SupportsDistinctFrom` |

## 6. Execution

```rust,no_run
# use rurqe::postgres::prelude::*;
# rurqe::table! {
#     User => "users" {
#         UserId => "id": i64,
#     }
# }
# fn main() -> Result<(), Box<dyn std::error::Error>> {
let mut conn: PgConnection = todo!();
// `.build()` assembles the AST and runs the Tier 2 checks before any I/O:
let built = select_from(User).select(cols!(UserId)).build()?;
let query = select_from(User).select(cols!(UserId));

let all = conn.fetch_all(&query)?;
let one = conn.fetch_one(&query)?; // 0 rows  -> ExecutionError::NoRows
let opt = conn.fetch_optional(&query)?; // >1 rows -> ExecutionError::MultipleRows
let n = conn.execute(&query)?; // affected rows
# let _ = (built, all, one, opt, n);
#     Ok(())
# }
```

`fetch_optional` reads at most two rows, so the whole set is never materialized.

Transactions:

```rust,no_run
# use rurqe::postgres::prelude::*;
# rurqe::table! {
#     User => "users" {
#         UserId => "id": i64,
#     }
# }
# fn main() -> Result<(), Box<dyn std::error::Error>> {
let mut conn: PgConnection = todo!();
let value = conn.transaction(|tx| {
    tx.execute(&insert_into(User).values((UserId::set(1i64),)))?;
    Ok(())
})?;
# let _ = value;

conn.transaction_with_isolation(IsolationLevel::Serializable, |tx| {
    tx.execute(&insert_into(User).values((UserId::set(2i64),)))?;
    Ok(())
})?;
#     Ok(())
# }
```

Rules:

- an outer block is `BEGIN` (`COMMIT` on success), a nested block is
  `SAVEPOINT rurqe_sp_N`;
- a panic or an exit without `commit()` rolls the block back;
- a driver refusal of `ROLLBACK` poisons the connection: the next `begin`
  retries the rollback first;
- the isolation level affects only the outer `BEGIN`.

## 7. Domain types

```rust,no_run
# use rurqe::prelude::*;
# #[derive(Clone, Copy, PartialEq, Eq, Debug)]
# pub struct Txid([u8; 32]);
# impl AsRef<[u8]> for Txid {
#     fn as_ref(&self) -> &[u8] {
#         &self.0
#     }
# }
# impl TryFrom<&[u8]> for Txid {
#     type Error = std::array::TryFromSliceError;
#     fn try_from(bytes: &[u8]) -> Result<Self, Self::Error> {
#         Ok(Txid(bytes.try_into()?))
#     }
# }
# #[derive(Clone, PartialEq, Eq, Debug)]
# pub struct Address(String);
# impl std::fmt::Display for Address {
#     fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
#         f.write_str(&self.0)
#     }
# }
# impl std::str::FromStr for Address {
#     type Err = std::convert::Infallible;
#     fn from_str(s: &str) -> Result<Self, Self::Err> {
#         Ok(Address(s.to_owned()))
#     }
# }
rurqe::impl_sql_binary_newtype!(Txid, 32);
rurqe::impl_sql_text_newtype!(Address);

#[derive(SqlType, Clone, Copy, PartialEq, Eq, Debug)]
pub struct UserId(i64); // #[sql(transparent)]
# let _ = (Txid([0u8; 32]), Address(String::new()), UserId(0));
```

Derive modes: `transparent`, `as = Bytes` (+ `len = N`), `as = Text`; for an
enum — the variant label with `#[sql(rename = "...")]`.

## 8. Cargo features

| Feature | What it adds |
|---|---|
| `postgres` | `PgConnection` (synchronous `postgres`) |
| `tokio-postgres` | `PgAsyncConnection`, `PgAsyncTransaction` |
| `deadpool-postgres` | `PgAsyncPool` |
| `r2d2` | `PgPool`, `SqlitePool` |
| `sqlite` | `Sqlite`, `Sqlite3_*`, `SqliteConnection` |
| `macros` | `select!`, `cols!`, `dto!`, `sql!`, `raw_sql!`, `SqlType` |
| `migrate` | `rurqe::migrate` |
| `decimal` | `SqlVal::Decimal`, `rust_decimal::Decimal` |
| `uuid` | `SqlVal::Uuid`, `uuid::Uuid` |
| `chrono` | `SqlVal::NaiveDateTime` / `DateTimeUtc` / `NaiveDate` |
| `serde_json` | `SqlVal::Json`, `Json<T>` |

## 9. Checklist before submitting code

- [ ] No turbofish `::<Postgres, …>` anywhere — the version prelude is used.
- [ ] Every table in the query is introduced through `from`/a join; a repeated
      one is aliased.
- [ ] `UPDATE`/`DELETE` carry `where_` or a deliberate `allow_all()`.
- [ ] Fields from a `LEFT JOIN` are read as `Option<T>`.
- [ ] With `group_by`, every projection is a group or an aggregate.
- [ ] `DISTINCT`/`DISTINCT ON` agree with `ORDER BY`.
- [ ] `RETURNING`/`ON CONFLICT`/`LATERAL` are used only on a version that has
      them.
- [ ] Domain types are described by a macro or the derive, not by `String`.

## 10. Symptom → cause

| Symptom | Cause | Fix |
|---|---|---|
| `table X is not part of this query's scope` | the table was not introduced into the query | `.from(X)` or a join |
| `column X does not belong to table Y` | a column of a foreign table was used | take a column of `Y` |
| `dialect … does not support RETURNING` | the version does not implement the capability | raise the engine version or drop `.returning` |
| `dialect … does not support row-level locking` | `.for_update()` on SQLite | move to PostgreSQL |
| ``cannot decode a SQL `text` value into `i64` `` | the column is mapped to another type | fix the codegen/override |
| `cannot decode a row of 1 values into a row type of 2 values` | the projection and the row type disagree | align `select` and `FromRow` |
| `duplicate unaliased relation …` | one effective name appears twice | alias it |
| `aggregate functions are not allowed in …` (pre-flight) | an aggregate in a row clause | move it into a subquery |
