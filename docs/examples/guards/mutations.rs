# Mutation guards

A mutation refuses to build before it renders any SQL when the statement records
no row source, assigns no column, or restricts no row, and a capability the
target engine lacks keeps its constructor out of reach at compile time. Each
fenced block below is one of those refusals, written as a `compile_fail`
doctest.

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

## The statement records no row source

```rust,compile_fail
// ANCHOR: insert_without_rows
use rurqe::postgres::prelude::*;

table!(
    /// The `users` table.
    User => "users" {
        UserId => "id": i64,
        UserName => "name": String,
        UserEmail => "email": String,
    }
);

fn main() {
    // Rejected: the statement names a table but records no row to insert.
    let written = insert_into(User).build();
}
// ANCHOR_END: insert_without_rows
```

## The target engine accepts no alias on the `INSERT` target

```rust,compile_fail
// ANCHOR: aliased_insert_target
use rurqe::sqlite::prelude::*;

fn assert_aliased_insert<D: Dialect + SupportsAliasedInsertTarget>() {}

fn main() {
    // Rejected: SQLite renders no alias production on the INSERT target.
    assert_aliased_insert::<Sqlite<Sqlite3_35>>();
}
// ANCHOR_END: aliased_insert_target
```

## The `UPDATE` restricts no row

```rust,compile_fail
// ANCHOR: unfiltered_update
use rurqe::postgres::prelude::*;

table!(
    /// The `users` table.
    User => "users" {
        UserId => "id": i64,
        UserName => "name": String,
    }
);

fn main() {
    // Rejected: the statement assigns a value but restricts no row.
    let mutating = update(User)
        .set(UserName::set("root"))
        .build();
}
// ANCHOR_END: unfiltered_update
```

## The `UPDATE` assigns no column

```rust,compile_fail
// ANCHOR: update_without_assignments
use rurqe::postgres::prelude::*;

table!(
    /// The `users` table.
    User => "users" {
        UserId => "id": i64,
        UserName => "name": String,
    }
);

fn main() {
    // Rejected: the statement restricts a row but assigns nothing to it.
    let mutating = update(User)
        .where_(UserId.eq(1i64))
        .build();
}
// ANCHOR_END: update_without_assignments
```

## The `DELETE` restricts no row

```rust,compile_fail
// ANCHOR: unfiltered_delete
use rurqe::postgres::prelude::*;

table!(
    /// The `users` table.
    User => "users" {
        UserId => "id": i64,
    }
);

fn main() {
    // Rejected: the statement removes rows but restricts none.
    let removed = delete_from(User).build();
}
// ANCHOR_END: unfiltered_delete
```
