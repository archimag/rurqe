# RURQE Philosophy

**RURQE** (**R**ust **U**niversal **R**elational **Q**uery **E**ngine) is a strictly typed SQL query builder, relational execution engine, and database-first code generation toolkit for Rust.

## The problem

The classic gap between application code and a relational database:

1. **The schema lives in two places.** DDL migrations describe one shape, application structs another, and the divergence surfaces at runtime (`column "..." does not exist`) instead of at build time.
2. **SQL is a string.** A string is not checked by the compiler: a typo in a column name, a reference to a table absent from `FROM`, a `LEFT JOIN` column read as a non-nullable value — every one of them is a runtime error.
3. **Types are lost at the boundary.** Rows come back as tuples without names, and a `NULL` produced by an outer join arrives in a field declared as `f64`.

RURQE answers with one invariant: **if a query compiles, it is semantically correct for the target dialect and engine version**.

## What that looks like

A report over two relations, written once and checked while it is built:

```rust,no_run
# extern crate rurqe;
# use rurqe::postgres::prelude::*;
# table! { User => "users" { UserId => "id": i64, UserName => "name": String, } }
# table! { Order => "orders" { OrderUserId => "user_id": i64, OrderTotal => "total": f64, } }
# fn main() -> Result<(), Box<dyn std::error::Error>> {
# let mut conn: PgConnection = todo!();
let feed = select_from(User)
    .left_join(Order, OrderUserId.eq(Expr::column::<UserId>()))
    .select(select!(
        user_id: UserId,
        user_name: UserName,
        total: OrderTotal,
    ))
    .order_by_desc(UserId)
    .limit(20)
    .fetch_all(&mut conn)?;

for row in feed {
    // `row.total` is `Option<f64>`: the LEFT JOIN lifted it, so a missing
    // relation can never be decoded into a plain `f64`.
}
# Ok(())
# }
```

No turbofish, no SQL literals, no manual mapping: the dialect is pinned by the prelude, the projection is named by `select!`, and the nullability of `row.total` follows from the join.

The `User` and `Order` markers come from `table!` in tests and small projects, and from `cargo db-codegen` in a real one — in both cases they are zero-sized values, so passing `User` around costs nothing.

A one-to-many relation is one statement as well — no N+1 queries and no Cartesian product folded by hand on the client:

```rust,no_run
# extern crate rurqe;
# use rurqe::postgres::prelude::*;
# table! { User => "users" { UserId => "id": i64, UserName => "name": String, } }
# table! { Order => "orders" { OrderId => "id": i64, OrderUserId => "user_id": i64, OrderTotal => "total": f64, } }
# fn main() -> Result<(), Box<dyn std::error::Error>> {
# let mut conn: PgConnection = todo!();
let users_with_orders = select_from(User)
    .select(select!(
        user_id: UserId,
        user_name: UserName,
        orders: nest_many(
            select_from(Order)
                .select(select!(order_id: OrderId, total: OrderTotal))
                .where_(OrderUserId.eq(UserId.without_scope()))
        ),
    ))
    .fetch_all(&mut conn)?;
# Ok(())
# }
```

The engine aggregates the child rows into a JSON document and the client decodes it into a `Vec` whose element type is the nested `select!` record — see [Nested results](../features/nested.md).

## Five principles

### 1. Code as SSOT (single source of truth)

The schema is extracted from the live database catalog (`pg_catalog`, `information_schema`, the SQLite `PRAGMA` tables) instead of being rewritten by hand. Zero-sized table and column markers, DTO structs, and the catalog snapshot `schema.lock` with its hash are generated from it, so a name in Rust and a name in the database cannot drift apart unnoticed.

The same principle governs this book: every claim about a compiler guarantee or a runtime behaviour is backed by a code location or an existing test.

### 2. Zero database rejection

No syntactically or semantically invalid query reaches the engine. The error is caught either by the compiler or by a deterministic check performed while the statement is assembled — see [Two-tier SQL correctness](two_tier.md).

### 3. Native-or-nothing

A capability the target engine cannot express natively is not emulated with client-side workarounds. `FOR UPDATE` on SQLite does not become "roughly the same thing": `.for_update()` simply does not exist there, and the code does not compile. `MERGE` follows the same policy — a synthetic `SELECT` + `INSERT/UPDATE` emulation creates races and is forbidden.

### 4. Zero-cost

The dialect is a zero-sized type parameter, table and column markers are ZSTs, and rendering is a single pass over the recorded statement into a `String` plus a parameter vector. No virtual calls, no reflection, no GC.

### 5. KISS, YAGNI, DRY

No "just in case" fields, no dead code kept "for later". Standard formats and cryptography use mature crates rather than hand-written parsers; the few exceptions are stated and justified.

## What this gives in practice

| Promise | How it feels |
|---|---|
| A column typo is a compile error | the message names the column and the SQL clause, never a trait cascade |
| A `NULL` from a `LEFT JOIN` cannot be read as `f64` | `Option<T>` appears in the row type on its own |
| An `UPDATE` without `WHERE` cannot run by accident | the statement does not build until `.where_(...)` or a deliberate `.allow_all()` |
| A construct the engine lacks does not compile | on SQLite `.for_update()` is not a method at all |
| An ambiguous `JOIN` never reaches the database | the statement is refused by the compiler, and the colliding relation is named |
| The schema never drifts from the code | `cargo db-codegen`, `schema.lock`, `cargo db-verify` |

## Where to go next

- [The architectural tetrad](tetrad.md) — the four industrial ideas the project is assembled from.
- [Two-tier SQL correctness](two_tier.md) — which gate catches which rule.
- [Quickstart in five minutes](quickstart.md) — the first working query.
- [Relational theory overview](../theory/overview.md) — optional reading for the maths behind the types.
