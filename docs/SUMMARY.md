# Getting Started

- [Introduction](intro.md)
- [Quickstart](quickstart.md)
- [Guided Tour](guided_tour.md)

# Querying

- [The SELECT statement](core/select.md)
  - [Joins](core/joins.md)
  - [Aliases](core/aliasing.md)
  - [Predicates and `WHERE`](core/filtering.md)
  - [Subqueries](core/subqueries.md)
  - [`GROUP BY` and `HAVING`](core/group_by.md)
  - [Window functions](core/window.md)
  - [Set Operations](core/set_ops.md)
  - [Ordering and pagination](core/ordering.md)
- [Expressions](core/expressions.md)
- [Projections and typed rows](core/projections.md)
- [The WITH statement](core/cte.md)
- [Nested Results (MULTISET)](core/nested.md)

# Data Modification

- [The INSERT statement](core/insert.md)
- [The UPDATE statement](core/update.md)
- [The DELETE statement](core/delete.md)

# Execution

- [SQL execution: sync vs async](core/execution.md)
- [Transactions](core/transactions.md)

# Schema & Code Generation

- [Database schema](core/schema.md)
- [Multi-schema databases](core/multi_schema.md)
- [Schema code generation](lifecycle/codegen.md)

# Schema Lifecycle

- [Database migrations](lifecycle/intro.md)
- [Sequential migrations](lifecycle/migrations.md)
- [Checkpoints and pruning](lifecycle/checkpoints.md)
- [Declarative schema](lifecycle/schema.md)
- [Running migrations in production](lifecycle/embedded.md)
- [Schema drift verification](lifecycle/drift.md)

# Reference

- [Two-tier SQL correctness](core/correctness.md)
- [Comparing PostgreSQL and SQLite](core/dialects/comparison.md)
- [PostgreSQL](core/dialects/postgres.md)
- [SQLite](core/dialects/sqlite.md)
- [Project configuration](lifecycle/config.md)
