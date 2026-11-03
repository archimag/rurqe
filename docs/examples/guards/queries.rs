# Query guards

A `SELECT` becomes renderable only once it records a projection, and the
compiler refuses a projection that names a column of a relation the statement
never introduces. Each fenced block below is one of those refusals, written as a
`compile_fail` doctest.

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

## The projection reads a relation the statement never introduces

```rust,compile_fail
// ANCHOR: out_of_scope_projection
use rurqe::postgres::prelude::*;

table!(
    /// The `users` table.
    User => "users" {
        UserId => "id": i64,
    }
);

table!(
    /// The `orders` table.
    Order => "orders" {
        OrderTotal => "total": f64,
    }
);

fn main() {
    // Rejected by the compiler: `OrderTotal` belongs to a table the query never introduces.
    let _ = select_from(User).select(cols!(UserId, OrderTotal));
}
// ANCHOR_END: out_of_scope_projection
```

## The statement records no projection

```rust,compile_fail
// ANCHOR: select_without_projection
use rurqe::postgres::prelude::*;

table!(
    /// The `users` table.
    User => "users" {
        UserId => "id": i64,
    }
);

fn main() {
    // Rejected: nothing is projected, so there is no statement to build.
    let statement = select_from(User).build();
}
// ANCHOR_END: select_without_projection
```

## The projection reads a relation the query never reads

```rust,compile_fail
// ANCHOR: select_out_of_scope_projection
use rurqe::postgres::prelude::*;

table!(
    /// The `users` table.
    User => "users" {
        UserId => "id": i64,
    }
);

table!(
    /// The `orders` table.
    Order => "orders" {
        OrderTotal => "total": f64,
    }
);

fn main() {
    // Rejected: `OrderTotal` belongs to `orders`, a relation this query never reads.
    let rows = select_from(User).select(cols!(UserId, OrderTotal));
}
// ANCHOR_END: select_out_of_scope_projection
```

## The statement publishes no relation

```rust,compile_fail
// ANCHOR: cte_without_projection
use rurqe::postgres::prelude::*;

table!(
    /// The `users` table.
    User => "users" {
        UserId => "id": i64,
    }
);

table!(
    /// The `user_totals` relation.
    UserTotals => "user_totals" {
        TotalsOwnerId => "owner_id": i64,
        TotalsTotal => "total": f64,
    }
);

fn main() {
    // Rejected: `user_totals` is named by the marker, but this statement records
    // no projection, so there is no statement to publish as a relation.
    let totals = select_from(User).as_cte::<UserTotals>();
}
// ANCHOR_END: cte_without_projection
```

## The published relation is read without being introduced

```rust,compile_fail
// ANCHOR: cte_out_of_scope_column
use rurqe::postgres::prelude::*;

table!(
    /// The `users` table.
    User => "users" {
        UserId => "id": i64,
    }
);

table!(
    /// The `user_totals` relation.
    UserTotals => "user_totals" {
        TotalsOwnerId => "owner_id": i64,
        TotalsTotal => "total": f64,
    }
);

fn main() {
    // Rejected: the query never reads `user_totals`, so the column is out of scope.
    let rows = select_from(User).select(cols!(UserId, TotalsTotal));
}
// ANCHOR_END: cte_out_of_scope_column
```
