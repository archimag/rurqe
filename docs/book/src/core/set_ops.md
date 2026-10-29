# Set operations

`UNION`, `UNION ALL`, `INTERSECT`, and `EXCEPT` combine the rows of two statements. The operators live on a projected query and consume the other operand entirely.

A set operation is the step of the [logical query processing order](select.md#logical-query-processing-order) that combines finished statements: each operand is projected first, and the combination then fixes the output columns of the whole statement. The head page of the statement family is [The `SELECT` statement](select.md).

## The family

| Method | SQL | Rows |
|---|---|---|
| `.union(other)` | `UNION` | the distinct rows of both |
| `.union_all(other)` | `UNION ALL` | all rows of both, duplicates kept |
| `.intersect(other)` | `INTERSECT` | the rows produced by both |
| `.except(other)` | `EXCEPT` | the rows of this query absent from `other` |

The two statements must share the dialect `D` and the output type `Output` — the signature fixes both, so a combined statement keeps decoding its rows with the same `FromRow`.

```rust,no_run
# extern crate rurqe;
# use rurqe::postgres::prelude::*;
# table! { User => "users" { UserName => "name": String, } }
# table! { ArchivedUser => "archived_users" { ArchivedUserName => "name": String, } }
# fn main() -> Result<(), Box<dyn std::error::Error>> {
let combined = select_from(User)
    .select(cols!(UserName))
    .union_all(
        select_from(ArchivedUser)
            .select(cols!(ArchivedUserName)),
    )
    .build()?;
# let _ = combined;
# Ok(())
# }
```

```text
(SELECT "name" FROM "users") UNION ALL (SELECT "name" FROM "archived_users")
```

## What the combination changes

- The combined rows no longer belong to a table, so every table-qualified method — `.where_` over table columns, `.order_by` with columns, joins — no longer applies.
- The projection is fixed by the first list: `.select(...)` is unreachable on the combined statement.
- A combined statement can neither keep the row-level lock of an operand nor accept a new one.
- `limit` / `offset` apply to the combined rows.

## Ordering the combined rows

Ordering the combined statement goes through the output-oriented forms: `order_by_output*` addresses an output column by name, and `order_by_index*` addresses it by a 1-based position.

```rust,no_run
# extern crate rurqe;
# use rurqe::postgres::prelude::*;
# table! { User => "users" { UserName => "name": String, } }
# table! { ArchivedUser => "archived_users" { ArchivedUserName => "name": String, } }
# fn main() -> Result<(), Box<dyn std::error::Error>> {
let page = select_from(User)
    .select(cols!(UserName))
    .union_all(select_from(ArchivedUser).select(cols!(ArchivedUserName)))
    .order_by_output_asc(UserName)
    .limit(50)
    .build()?;
# let _ = page;
# Ok(())
# }
```

A criterion recorded **before** the set operation belongs to the left operand and renders inside its parentheses; a criterion recorded **after** it applies to the combined rows. The first combined-row criterion closes the statement: no further operand can follow the ordering of the combined rows.

## What `.build()` adds

The rules that compare assembled clauses rather than types run pre-flight:

- `BuildError::CompoundOrderIndex` — a 1-based position outside the projection list;
- `BuildError::CompoundOrderColumn` — an output name the projection list does not produce;
- `BuildError::SetOperandArity` — an operand disagreeing with the assembled clause; the type signature keeps it out of user code, and the rule guards ASTs assembled without it.

## Related chapters

- [The `SELECT` statement](select.md) — the clause anatomy and the evaluation order this chapter is a step of.
- [Ordering and pagination](ordering.md) — the per-operand ordering.
- [Projections and `SELECT`](select.md) — how the output columns are fixed.
