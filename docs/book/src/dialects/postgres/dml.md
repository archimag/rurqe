# DML: RETURNING, ON CONFLICT, INSERT … SELECT

## RETURNING

`returning(…)` exists on `INSERT`, `UPDATE`, and `DELETE` for every dialect
implementing `SupportsReturning` — for PostgreSQL that is every engine
version. It projects single columns, tuples, or whole record selections,
decoded like any `SELECT` projection:

```rust,no_run
# extern crate rurqe;
use rurqe::postgres::prelude::*;
# table! { User => "users" { UserId => "id": i64, UserEmail => "email": String, UserActive => "is_active": bool, } }
# fn main() {
let inserted = insert_into(User)
    .values((
        UserEmail::set("user@example.com"),
        UserActive::set(true),
    ))
    .returning(cols!(UserId))
    .build()
    .expect("valid insert");
# let _ = inserted;
# }
```

## ON CONFLICT

Upserts are gated by `SupportsOnConflict`. The conflict target names the
constrained columns, `do_nothing()` skips the conflicting row, and
`do_update(…)` assigns under the conflict:

```rust,no_run
# extern crate rurqe;
use rurqe::postgres::prelude::*;
# table! { User => "users" { UserId => "id": i64, UserName => "name": String, } }
# fn main() {
let upsert = insert_into(User)
    .values((UserId::set(1i64), UserName::set("alice")))
    .on_conflict(UserId)
    .do_update((UserName.set(excluded(UserName)),))
    .build()
    .expect("valid upsert");
# let _ = upsert;
# }
```

* `target_where(…)` narrows the conflict target to rows matching a predicate —
  the partial-unique-index form, rendered as
  `ON CONFLICT (…) WHERE …`.
* `update_where(…)` adds the `WHERE` clause of `DO UPDATE`, rendered as
  `… DO UPDATE SET … WHERE …` — only rows matching the predicate update.
* `excluded(col)` references the rejected row's value as a *typed* column:
  it renders `"EXCLUDED"."col"` and type-checks against the column it wraps,
  so an assignment can never read the wrong kind of value from the rejected
  row.

Both conflict predicates render natively — no `CASE WHEN` emulation, no
client round-trip.

## INSERT INTO … SELECT

A compatible `SelectQuery` inserts directly into a target table, server-side,
without a client round-trip; parameters chain across both statements and the
projection type-checks against the target columns:

```rust,no_run
# extern crate rurqe;
use rurqe::postgres::prelude::*;
# table! { User => "users" { UserId => "id": i64, UserEmail => "email": String, UserActive => "is_active": bool, } }
# table! { ArchivedUser => "archived_users" { ArchivedUserId => "id": i64, ArchivedUserEmail => "email": String, } }
# fn main() {
let archive = insert_into(ArchivedUser)
    .columns((ArchivedUserId, ArchivedUserEmail))
    .from_select(
        select_from(User)
            .select((UserId, UserEmail))
            .where_(UserActive.eq(false)),
    )
    .build()
    .expect("valid insert from select");
# let _ = archive;
# }
```

An empty row source is rejected — an insert with no rows is a typestate
violation, not a silent no-op.
