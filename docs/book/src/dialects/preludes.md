# Preludes

Queries are generic over their dialect, but an application should name that
dialect exactly once. Each dialect facade ships a prelude that binds it for
the whole import, so the rest of the code carries no type annotations:

```rust,no_run
# extern crate rurqe;
use rurqe::postgres::prelude::*;
# table! { User => "users" { UserId => "id": i64, UserName => "name": String, UserActive => "active": bool, } }
# fn main() {
{{#rustdoc_include ../../../../examples/dialects.rs:postgres}}
# }
```

A prelude exports the dialect-bound query entry points — `select_from`,
`insert_into`, `update`, `delete_from` — together with the expression
traits and the projection macros the entry points produce.

## Version-bound preludes

The same pattern applies one level down: each engine version ships its own
prelude — for example `rurqe::postgres::pg14::prelude` or
`rurqe::sqlite::sqlite3_24::prelude` — which pins the version and with it
every capability gate of that version. A module importing the 3.24 prelude
sees no `RETURNING` methods at all, because the bound version does not
satisfy `SupportsReturning`.

## Connection-bound factories

The dialect can equally be inferred from a database handle: the connection
and transaction types expose `select_from(…)`, `update(…)`, and the other
entry points as methods, so a query built from `conn` picks up the dialect of
the connection without any annotation.

Import exactly one prelude per module: two dialects in scope leave the entry
points ambiguous, and the compiler says so at the first call.
