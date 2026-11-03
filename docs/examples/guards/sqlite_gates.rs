# SQLite capability guards

A clause a release of the engine does not implement has no builder method in the
module pinned to a release that predates it, so the statement is rejected by the
compiler rather than by the engine. Each fenced block below is one of those
refusals, written as a `compile_fail` doctest.

Every block is self-contained — the prelude import, the table declarations, and
the offending statement — so it fails for the rejection it documents rather than
for a marker it never declared. The file is attached to the crate through
`src/guards.rs`, a catalogue module compiled only while rustdoc gathers
doctests, so `cargo test --doc` runs each block as an expected failure: a guard
that stops refusing fails the suite instead of leaving a chapter that quotes a
rejection nothing produces any more.

A chapter quotes a region of this file inside its own `rust,compile_fail` fence
rather than restating the rejection inline, so the diagnostic it shows cannot
drift from the code that produces it.

## The engine has no per-row locking

SQLite locks the whole database file for the duration of a write, so it has no
per-row locking and the locking methods are absent from the builder.

```rust,compile_fail
// ANCHOR: row_locking
use rurqe::sqlite::prelude::*;

table!(
    /// The `jobs` table.
    Job => "jobs" {
        JobId => "id": i64,
    }
);

fn main() {
    // Rejected at compile time: SQLite has no per-row locking, so the clause has
    // no builder method.
    let _ = select_from(Job)
        .select(cols!(JobId))
        .for_update()
        .skip_locked();
}
// ANCHOR_END: row_locking
```

## The engine has no `DISTINCT ON` clause

`DISTINCT ON` is a PostgreSQL clause, so the builder offers no such method on
this dialect.

```rust,compile_fail
// ANCHOR: distinct_on
use rurqe::sqlite::prelude::*;

table!(
    /// The `users` table.
    User => "users" {
        UserId => "id": i64,
    }
);

fn main() {
    // Rejected at compile time: DISTINCT ON is not a clause SQLite implements.
    let _ = select_from(User).select(cols!(UserId)).distinct_on(UserId);
}
// ANCHOR_END: distinct_on
```

## `ON CONFLICT` below the release that introduced it

SQLite gained `ON CONFLICT` in 3.24, so the clause has no builder method in the
module pinned to 3.0.

```rust,compile_fail
// ANCHOR: on_conflict_below_threshold
use rurqe::sqlite::sqlite3_0::prelude::*;

table!(
    /// The `users` table.
    User => "users" {
        UserId => "id": i64,
    }
);

fn main() {
    // Rejected at compile time: ON CONFLICT requires SQLite 3.24+.
    let _ = insert_into(User)
        .values(UserId::set(1i64))
        .on_conflict_do_nothing();
}
// ANCHOR_END: on_conflict_below_threshold
```

## `RETURNING` below the release that introduced it

`RETURNING` arrived in SQLite 3.35, so the clause has no builder method in the
module pinned to 3.24.

```rust,compile_fail
// ANCHOR: returning_below_threshold
use rurqe::sqlite::sqlite3_24::prelude::*;

table!(
    /// The `users` table.
    User => "users" {
        UserId => "id": i64,
    }
);

fn main() {
    // Rejected at compile time: RETURNING requires SQLite 3.35+.
    let _ = delete_from(User).allow_all().returning(cols!(UserId));
}
// ANCHOR_END: returning_below_threshold
```

## A nested collection below the release that bundles the JSON functions

A nested collection is assembled from the JSON functions, which were an opt-in
build option until SQLite 3.38, so the projection is rejected below that
release.

```rust,compile_fail
// ANCHOR: nested_collection_below_threshold
use rurqe::sqlite::sqlite3_35::prelude::*;

table!(
    /// The `users` table.
    User => "users" {
        UserId => "id": i64,
    }
);

table!(
    /// The `orders` table.
    Order => "orders" {
        OrderId => "id": i64,
        OrderUserId => "user_id": i64,
    }
);

fn main() {
    // Rejected at compile time: the JSON functions a nested collection renders
    // with are part of a default build only from SQLite 3.38, so the projection
    // is rejected below that release.
    let _ = select_from(User)
        .select(cols! {
            user_id: UserId,
            orders: nest_many(select_from(Order).select(cols!(order_id: OrderId))),
        });
}
// ANCHOR_END: nested_collection_below_threshold
```
