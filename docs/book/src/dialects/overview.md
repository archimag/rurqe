# Dialects

RURQE renders one typed query core into several SQL dialects. Two engines are
supported today — PostgreSQL through `rurqe-postgres` and SQLite through
`rurqe-sqlite` — and both are driven by the same builders, so the query you
already wrote targets the other engine as soon as the import changes.

```rust
# extern crate rurqe;
use rurqe::prelude::*;

table! {
    /// The `users` table.
    User => "users" {
        UserId => "id": i64,
        UserName => "name": String,
        UserActive => "active": bool,
    }
}
# fn main() {}
```

## The same query, both engines

```rust,no_run
# extern crate rurqe;
use rurqe::postgres::prelude::*;
# table! { User => "users" { UserId => "id": i64, UserName => "name": String, UserActive => "active": bool, } }
# fn main() {
let query = select_from(User)
    .select(select!(user_id: UserId, user_name: UserName))
    .where_(UserActive.eq(true))
    .order_by_asc(UserId);
# let _ = query;
# }
```

```sql
SELECT "id" AS "user_id", "name" AS "user_name" FROM "users"
WHERE ("active" = $1)
ORDER BY "id" ASC
```

The identical lines under `use rurqe::sqlite::prelude::*;` render the same
statement against SQLite:

```sql
SELECT "id" AS "user_id", "name" AS "user_name" FROM "users"
WHERE ("active" = ?)
ORDER BY "id" ASC
```

Only spelling changed: identifiers are quoted the way the engine quotes them,
and parameters are numbered the way it numbers them — `$1` for PostgreSQL, `?`
for SQLite. Spelling never changes what a query means, and it is the whole job
of the `Dialect` trait.

## Two things are not spelling

### Case-insensitive search works on both

`ILIKE` is a PostgreSQL extension; SQLite knows only the case-sensitive `LIKE`.
The same predicate keeps its meaning on both engines:

```rust,no_run
# extern crate rurqe;
# use rurqe::postgres::prelude::*;
# table! { User => "users" { UserName => "name": String, } }
# fn main() {
# let query = select_from(User)
#     .select(cols!(UserName))
.where_(UserName.ilike("%ali%"));
# let _ = query;
# }
```

```sql
-- PostgreSQL
("name" ILIKE $1)

-- SQLite
(LOWER("name") LIKE LOWER(?))
```

`LOWER` folds ASCII letters only, which keeps the fallback independent of the
database collation.

### Nested collections work on both

A `nest_many` / `nest_one` projection stays one statement on either engine, but
the SQL differs. PostgreSQL aggregates natively with `jsonb_agg(to_jsonb(…))`,
so the ordering and row limit of the nested statement shape the document;
SQLite assembles every object with `json_object` and collects them with
`json_group_array`. Both produce the same JSON document, so the client decodes
one shape whichever engine produced it.

SQLite shipped the JSON functions as an opt-in build option until 3.38, so the
projection is reachable from `sqlite3_38` only — see
[Nested results](../features/nested.md).

## Capabilities are proven per version

A clause is reachable only when the pinned engine version really runs it, so
`RETURNING` compiles against SQLite 3.35:

```rust,no_run
# extern crate rurqe;
use rurqe::sqlite::sqlite3_35::prelude::*;
# table! { User => "users" { UserId => "id": i64, UserName => "name": String, } }
# fn main() -> Result<(), Box<dyn std::error::Error>> {
# let mut db: SqliteConnection = todo!();
let row = db.fetch_one(&insert_into(User)
    .values((UserId::set(1i64), UserName::set("alice")))
    .returning(cols!(UserId, UserName)))?;
# let _ = row;
# Ok(())
# }
```

and against 3.24 it is not a method at all:

```rust
```rust
# extern crate rurqe;
use rurqe::sqlite::sqlite3_24::prelude::*;
# table! { User => "users" { UserId => "id": i64, UserName => "name": String, } }
# fn main() {

// The pinned version has no `RETURNING`: the call does not exist.
let inserted = insert_into(User)
    .values((UserId::set(1i64), UserName::set("alice")));
# let _ = inserted;
# }
```
Version pins are listed in [Versions](versions.md), the gates themselves in
[Capabilities](capabilities.md), and the way to bind one per module in
[Preludes](preludes.md).

## Native or nothing

A query either renders as SQL the target engine runs natively, or it does not
exist for that dialect. The refusal happens at three well-defined points:

1. **Compile time.** A clause the pinned version lacks is not a method, a
   column whose table is not in scope is not projectable, and `.for_update()`
   on a dialect without row locking cannot be named. The diagnostic speaks in
   SQL terms — it names the capability the dialect is missing.
2. **`.build()`.** Rules that must inspect the assembled statement run
   deterministically before any I/O: two relations sharing one effective name
   fail with `BuildError::DuplicateRelation`, a `DISTINCT ON` whose leading
   `ORDER BY` expressions do not match the `distinct_on` prefix fails with
   `BuildError::DistinctOnOrderMismatch`, and a `DISTINCT` query whose ordering
   names no projected expression fails with
   `BuildError::DistinctOrderNotProjected`.
3. **Runtime, as data.** What only the engine can decide — a syntax error
   inside `sql!`, a constraint violation, a decode mismatch — surfaces as a
   typed error through `Result`. Panics are forbidden on every path, including
   malformed raw SQL.

Nothing in between synthesizes a missing server feature: no `MERGE` assembled
from `INSERT` plus `UPDATE`, no `FOR UPDATE` emulated over a whole-database
lock, no `FULL OUTER JOIN` stitched together from outer joins.

## In this chapter

* [Philosophy: native or nothing](philosophy.md)
* [Capability markers](capabilities.md)
* [Engine versions](versions.md)
* [Preludes](preludes.md)
* [PostgreSQL](postgres/overview.md) — the dialect, its extensions, its drivers.
* [SQLite](sqlite/overview.md) — the dialect, its version thresholds, the
  `rusqlite` driver.
