# PostgreSQL capabilities

Every capability marker of `rurqe_core::dialect` is implemented for
`Postgres<V>` — unconditionally, for every engine version `V: PgVersion`:

```rust
impl<V: PgVersion> SupportsReturning for Postgres<V> {}
impl<V: PgVersion> SupportsDistinctOn for Postgres<V> {}
impl<V: PgVersion> SupportsOnConflict for Postgres<V> {}
impl<V: PgVersion> SupportsLateralJoin for Postgres<V> {}
impl<V: PgVersion> SupportsDeleteUsing for Postgres<V> {}
impl<V: PgVersion> SupportsUpdateFrom for Postgres<V> {}
impl<V: PgVersion> SupportsFullOuterJoin for Postgres<V> {}
impl<V: PgVersion> SupportsRowLocking for Postgres<V> {}
impl<V: PgVersion> SupportsAggregateFilter for Postgres<V> {}
impl<V: PgVersion> SupportsWindowFunctions for Postgres<V> {}
impl<V: PgVersion> SupportsNullsOrdering for Postgres<V> {}
impl<V: PgVersion> SupportsGreatest for Postgres<V> {}
impl<V: PgVersion> SupportsJsonAggregation for Postgres<V> {}
impl<V: PgVersion> SupportsDistinctFrom for Postgres<V> {}
```

The consequence: no core clause is withheld because an engine is "too old"
among the declared markers — `RETURNING`, upserts, `DISTINCT ON`,
`LATERAL`, `UPDATE … FROM`, `DELETE … USING`, `FULL OUTER JOIN`, row
locking, aggregate `FILTER`, window functions, explicit null ordering,
`greatest`/`least`, JSON aggregation, and `IS DISTINCT FROM` all exist for
`Pg14`, `Pg15`, `Pg16`, and the default parameter alike.

## Pinned by tests

The matrix is not documentation only — `crates/rurqe-postgres/src/dialect.rs`
holds it in three tests:

* `capabilities_are_satisfied` asserts the statement-level markers
  (`SupportsReturning`, `SupportsDistinctOn`, `SupportsOnConflict`,
  `SupportsLateralJoin`, `SupportsDeleteUsing`, `SupportsUpdateFrom`,
  `SupportsFullOuterJoin`, `SupportsAggregateFilter`) for `Pg14`, `Pg15`,
  `Pg16`, and `Postgres` without an annotation.
* `expression_capabilities_hold_for_every_version` does the same for the
  expression-level markers (`SupportsWindowFunctions`,
  `SupportsNullsOrdering`, `SupportsGreatest`, `SupportsJsonAggregation`,
  `SupportsDistinctFrom`).
* `row_level_locking_is_a_postgres_capability` asserts
  `SupportsRowLocking` for every version marker.

Each test compiles only while the corresponding `impl` blocks exist, so
removing a capability breaks the build, not a query in production.

## Where each clause is described

| Capability | Chapter |
|---|---|
| `SupportsReturning` | [DML](dml.md) |
| `SupportsOnConflict` | [DML](dml.md) |
| `SupportsRowLocking` | [Row-level locking](locking.md) |
| `SupportsLateralJoin` | [LATERAL joins](lateral.md) |
| `SupportsJsonAggregation` | [JSON and JSONB](json.md) |
| `SupportsWindowFunctions`, `SupportsAggregateFilter`, `SupportsNullsOrdering`, `SupportsGreatest` | core chapters: [window](../../core/window.md), [group by](../../core/group_by.md), [ordering](../../core/ordering.md) |
| `SupportsDistinctOn`, `SupportsDistinctFrom`, `SupportsDeleteUsing`, `SupportsUpdateFrom`, `SupportsFullOuterJoin` | the catalogue in [dialects/capabilities](../capabilities.md) |
