# RURQE

**RURQE** (**R**ust **U**niversal **R**elational **Q**uery **E**ngine) is an ergonomic, strictly type-safe SQL query builder, relational execution engine, and database-first codegen toolkit for Rust.

## Key Features

- **Zero-Boilerplate Ergonomics:** Fluent query syntax and ad-hoc named record projections via `select!` with no generic turbofish noise.
- **Two-Tier SQL Correctness:** Strict compile-time relational scope and capability enforcement (Tier 1) combined with deterministic pre-flight AST validation in `.build()` (Tier 2).
- **Automatic Sound Nullability:** Columns participating in outer joins automatically lift to `Option<T>` at compile time.
- **Database-First Single Source of Truth:** Schema metadata is introspected directly from live database catalogs (`PostgreSQL`, `SQLite`), generating typed schema markers and canonical `schema.lock` snapshots.
- **Stage-Based Evolution:** Declarative target DDL (`schema/`) paired with sequential migration delta tails (`migrations/`), verified mathematically via the Golden Equation in CI.
- **Runtime Orthogonality:** Queries are constructed identically for asynchronous (`tokio-postgres`) and synchronous (`postgres`, `rusqlite`) drivers, choosing runtime only at the terminal call.

## Quick Example

```rust
use rurqe::postgres::prelude::*;

// Assuming generated schema markers Users, Orders
let active_users = select! {
    user_id: Users::Id,
    user_name: Users::Name,
    order_total: Orders::Total, // Automatically Option<f64> via LEFT JOIN
}
.from(Users)
.left_join(Orders, Orders::UserId.eq(Users::Id))
.where_(Users::IsActive.eq(true))
.fetch_all(&pool)
.await?;
```

## Development & Tooling

This project is engineered using [TAUQE](https://github.com/archimag/tauqe), an AI-native engineering control environment built on the literal harness paradigm.

## Contributing

Pull requests containing raw code are not accepted. In the modern LLM paradigm, synthesizing raw code carries negligible marginal cost—code is no longer the scarce resource. What truly matters is conceptual clarity, invariant design, and cohesive system architecture.

Not a single line of code in this repository was written by a human, and never will be.

## License

Licensed under the [MIT License](LICENSE).
