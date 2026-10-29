# Summary

# Part I: Getting Started

- [Philosophy of RURQE](intro/philosophy.md)
- [The architectural tetrad](intro/tetrad.md)
- [Two-tier SQL correctness](intro/two_tier.md)
- [Quickstart in five minutes](intro/quickstart.md)

# Part II: Ergonomics & Execution

- [Everyday queries: the portable core](core/overview.md)
- [Schema: tables and columns](core/schema.md)
- [The `SELECT` statement](core/select.md)
  - [Predicates and `WHERE`](core/filtering.md)
  - [Expressions](core/expressions.md)
  - [Joins](core/joins.md)
  - [Aliases](core/aliasing.md)
  - [Subqueries](core/subqueries.md)
  - [`GROUP BY` and `HAVING`](core/group_by.md)
  - [Window functions](core/window.md)
  - [Ordering and pagination](core/ordering.md)
  - [Set operations](core/set_ops.md)
- [DML: `INSERT`](core/insert.md)
- [DML: `UPDATE`](core/update.md)
- [DML: `DELETE`](core/delete.md)
- [Common table expressions (`WITH`)](core/cte.md)
- [Nested results: `nest_many` and `nest_one`](features/nested.md)
  - [MULTISET: how nesting becomes SQL](features/multiset.md)
  - [Engines, capability gates, and decoding](features/multiset_engines.md)

# Part III: Dialects & Runtime

- [Dialects overview](dialects/overview.md)
- [Philosophy: native or nothing](dialects/philosophy.md)
- [Capability markers](dialects/capabilities.md)
- [Engine versions](dialects/versions.md)
- [Preludes](dialects/preludes.md)
- [PostgreSQL](dialects/postgres/overview.md)
  - [Capabilities](dialects/postgres/capabilities.md)
  - [DML: RETURNING, ON CONFLICT](dialects/postgres/dml.md)
  - [Row locking](dialects/postgres/locking.md)
  - [LATERAL joins](dialects/postgres/lateral.md)
  - [JSON and JSONB](dialects/postgres/json.md)
  - [Arrays](dialects/postgres/arrays.md)
  - [Full-text search](dialects/postgres/fts.md)
  - [Regular expressions](dialects/postgres/regex.md)
  - [Enums](dialects/postgres/enums.md)
  - [Drivers](dialects/postgres/drivers.md)
- [SQLite](dialects/sqlite/overview.md)
  - [Capabilities and version thresholds](dialects/sqlite/capabilities.md)
  - [Upsert: ON CONFLICT](dialects/sqlite/upsert.md)
  - [Aggregates, windows, ordering](dialects/sqlite/filter.md)
  - [RETURNING and UPDATE … FROM](dialects/sqlite/returning.md)
  - [Limitations without emulation](dialects/sqlite/limitations.md)
  - [JSON and nested collections](dialects/sqlite/json.md)
  - [Enums through CHECK](dialects/sqlite/enums.md)
  - [The rusqlite driver](dialects/sqlite/driver.md)
- [Runtime: overview](runtime/overview.md)
  - [Transactions and savepoints](runtime/transactions.md)
  - [Connection pools](runtime/pools.md)
  - [Ecosystem types, values, and decoding](runtime/types.md)
  - [Custom types](runtime/custom_types.md)

# Part IV: Lifecycle and Migrations

- [Schema evolution: overview](migrations/overview.md)
- [The crisis of traditional migrations in the AI era](migrations/problem.md)
- [Two modes and how to choose](migrations/modes.md)
- [The traditional path: `V...` and `R...`](migrations/traditional.md)
- [Stage + Tail: the declarative stage and cold start](migrations/stage_tail.md)
- [The Golden Equation](migrations/golden_equation.md)
- [Safe history pruning](migrations/pruning.md)
- [The checkpoint protocol](migrations/checkpoint_protocol.md)
- [Migration concurrency](dialects/sqlite/concurrency.md)

# Part V: The `rurqe` Toolchain

- [The `rurqe` toolchain](toolchain/overview.md)

# Part VI: Relational Theory

- [Overview: relational theory in Rust types](theory/overview.md)
- [From Codd's relational algebra to the Rust type system](theory/codd_to_rust.md)
- [Scopes: `TableScope` and `ExprScope`](theory/scopes.md)
- [Nullability: outer joins as a type change](theory/nullability.md)
- [Disjointness: relation name uniqueness](theory/disjointness.md)
- [NF² and MULTISET: nested relations](theory/nf2_multiset.md)

# Appendices

- [Appendices](appendix/overview.md)
- [LLM cheatsheet / prompt pack](appendix/llm_cheatsheet.md)
- [Compiler error reference](appendix/compiler_errors.md)
- [Capability matrix](appendix/matrix.md)
