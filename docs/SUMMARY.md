# Summary

- [Introduction](intro.md)
- [Getting Started](quickstart.md)

# Part I: Architecture & Concepts

- [Two-tier SQL correctness](core/correctness.md)
- [Database schema](core/schema.md)
- [Multi-schema databases](core/multi_schema.md)
- [Projections and typed rows](core/projections.md)
- [SQL execution: sync vs async](core/execution.md)
- [Transactions](core/transactions.md)

# Part II: SQL Statements

- [Expressions](core/expressions.md)
- [The WITH statement](core/cte.md)
- [The SELECT statement](core/select.md)
  - [Joins](core/joins.md)
  - [Aliases](core/aliasing.md)
  - [Predicates and `WHERE`](core/filtering.md)
  - [Subqueries](core/subqueries.md)
  - [`GROUP BY` and `HAVING`](core/group_by.md)
  - [Window functions](core/window.md)
  - [Set Operations](core/set_ops.md)
  - [Ordering and pagination](core/ordering.md)
- [The INSERT statement](core/insert.md)
- [The UPDATE statement](core/update.md)
- [The DELETE statement](core/delete.md)
- [The MULTISET statement](core/multiset.md)
  - [Nested results: `nest_many` and `nest_one`](core/nested.md)
  - [Engines, capability gates, and decoding](core/multiset_engines.md)

# Part III: Database Dialects

- [Comparing PostgreSQL and SQLite](core/dialects/comparison.md)
- [PostgreSQL](core/dialects/postgres.md)
- [SQLite](core/dialects/sqlite.md)

# Part IV: Schema Lifecycle & Tooling

- [Database migrations](lifecycle/database_migrations.md)
  - [Migrations](lifecycle/migrations.md)
  - [Running migrations in production](lifecycle/embedded.md)
  - [Stage-based schema evolution](lifecycle/stage.md)
  - [Schema drift verification](lifecycle/drift.md)
- [Schema code generation](lifecycle/codegen.md)
