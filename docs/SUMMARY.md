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
- [Nested results: `nest_many` and `nest_one`](core/nested.md)
  - [MULTISET: how nesting becomes SQL](core/multiset.md)
  - [Engines, capability gates, and decoding](core/multiset_engines.md)
