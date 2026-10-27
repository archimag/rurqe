# Capabilities

Each engine-dependent clause is gated by a marker trait in
`rurqe_core::dialect`. The builder method that would emit the clause is
bounded by the trait, so on a dialect without the capability the method does
not exist and the violation is a compile-time error at the call site.

## Marker catalogue

| Marker | Gates | PostgreSQL | SQLite |
|---|---|---|---|
| `SupportsReturning` | `RETURNING` on DML | every version | 3.35+ |
| `SupportsDistinctOn` | `DISTINCT ON (…)` | every version | — |
| `SupportsOnConflict` | `ON CONFLICT` upserts | every version | 3.24+ |
| `SupportsLateralJoin` | `LATERAL` joins | every version | — |
| `SupportsDeleteUsing` | `DELETE … USING` | every version | — |
| `SupportsUpdateFrom` | `UPDATE … FROM` | every version | 3.33+ |
| `SupportsFullOuterJoin` | `FULL OUTER JOIN` | every version | — |
| `SupportsRowLocking` | `FOR UPDATE` / `FOR SHARE` (+ `NOWAIT`, `SKIP LOCKED`) | every version | — |
| `SupportsAggregateFilter` | aggregate `FILTER (WHERE …)` | every version | 3.30+ |
| `SupportsWindowFunctions` | `OVER (…)` window clauses | every version | 3.25+ |
| `SupportsNullsOrdering` | explicit `NULLS FIRST` / `NULLS LAST` | every version | 3.30+ |
| `SupportsGreatest` | `greatest(…)` / `least(…)` | every version | — |
| `SupportsJsonAggregation` | JSON aggregation of nested row sets | every version | 3.38+ |
| `SupportsDistinctFrom` | `IS [NOT] DISTINCT FROM` | every version | — |

The PostgreSQL column holds for every `PgVersion` marker — see
[PostgreSQL capabilities](postgres/capabilities.md). The SQLite column names
the engine version where the feature appears: the dialect exposes the
capability only when the selected version marker meets the threshold, so
`RETURNING` exists for `Sqlite<Sqlite3_35>` and not for
`Sqlite<Sqlite3_24>`, while `FULL OUTER JOIN` (engine: 3.39.0) sits past
every declared marker and is unavailable at any of them.

Two more dialect-wide flags live on the `Dialect` trait itself rather than
in markers, because they change *rendering* rather than availability:
`SUPPORTS_ILIKE` (render `ILIKE` directly, or fold both operands with
`LOWER`) and `SUPPORTS_JSONB_ROW_AGG` (`jsonb_agg(to_jsonb(…))` versus
field-by-field `json_object` + `json_group_array`) — see the
[overview](overview.md#the-dialect-contract).

## Rules proven during `.build()`

Some rules need to compare clauses with each other, or to assert that
something is *absent* from the statement. Stable Rust cannot express a
negative assertion in the trait system, so these run deterministically inside
`.build()`, before any network I/O, and fail with a typed `BuildError`:

| Rule | Error | Catches |
|---|---|---|
| two relations in `FROM`/`JOIN` share one effective name | `DuplicateRelation { name }` | unaliased duplicate joins; a relation and an alias colliding with it |
| `DISTINCT ON` prefix must lead the `ORDER BY` | `DistinctOnOrderMismatch { position }` | PostgreSQL prefix alignment, left to right |
| `DISTINCT` requires `ORDER BY` expressions to be projected | `DistinctOrderNotProjected { position }` | ordering an expression the projection does not name |
| compound `ORDER BY` position must lie inside the projection | `CompoundOrderIndex` | a `UNION`-combined statement ordered by a 1-based position outside the projection |
| compound `ORDER BY` name must be produced by the projection | `CompoundOrderColumn` | ordering the combined rows by an output name neither operand produces |

The named relations participate under their *effective* name — the alias a
relation is referenced under, or its table name when it carries none — so
derived tables and lateral subqueries collide exactly like plain tables.
The remedy for a genuine self-join is an alias; the error message says so.
