# Dialects

RURQE renders one typed query core into several SQL dialects. Two engines are
supported today — PostgreSQL through `rurqe-postgres` and SQLite through
`rurqe-sqlite` — and both are driven by the same builder types. What differs
between them splits into two dimensions that the API keeps separate:

* **Rendering rules** — how identifiers are quoted and how parameter
  placeholders are numbered — live in the `Dialect` trait and never change
  the meaning of a query.
* **Capabilities** — which clauses and operators an engine understands at
  a given version — live in marker traits such as `SupportsReturning`,
  proven per dialect *and per engine version*.

## Native or nothing

A query either renders as SQL the target engine runs natively, or it does not
exist for that dialect. The refusal happens at three well-defined points:

1. **Compile time.** Everything the type system can prove is rejected by
   `rustc`: a column whose table is not in scope, a bare column projected out
   of a `GROUP BY`, `.for_update()` on a dialect without
   `SupportsRowLocking`. The unsupported method does not exist for that
   dialect, so the mistake is a type error at the call site.
2. **`.build()`.** Rules that must inspect the assembled statement run
   deterministically before any network I/O: two relations sharing one
   effective name in `FROM`/`JOIN` fail with `BuildError::DuplicateRelation`,
   a `DISTINCT ON` whose leading `ORDER BY` expressions do not match the
   `distinct_on` prefix fails with `BuildError::DistinctOnOrderMismatch`, and
   a `DISTINCT` query whose ordering names no projected expression fails
   with `BuildError::DistinctOrderNotProjected`.
3. **Runtime, as data.** What only the engine can decide — a syntax error
   inside `sql!`, a constraint violation, a decode mismatch — surfaces as a
   typed error through `Result`. Panics are forbidden on every path,
   including malformed raw SQL.

Nothing in between synthesizes a missing server feature: no `MERGE` assembled
from `INSERT` plus `UPDATE`, no `FOR UPDATE` emulated over SQLite's
database-level lock, no `FULL OUTER JOIN` stitched together from outer joins.

## The `Dialect` contract

```rust
pub trait Dialect: 'static + Send + Sync {
    /// Whether the dialect renders `ILIKE` directly.
    const SUPPORTS_ILIKE: bool = true;
    /// Whether the dialect aggregates a nested row set into native JSON.
    const SUPPORTS_JSONB_ROW_AGG: bool = false;

    /// Appends a quoted identifier to the output SQL buffer.
    fn push_quoted_identifier(out: &mut String, ident: &str);

    /// Appends a parameter placeholder for a 1-based parameter index.
    fn push_placeholder(out: &mut String, index: usize);

    /// Returns a newly allocated quoted identifier string.
    fn quote_identifier(ident: &str) -> String { /* … */ }

    /// Returns a newly allocated placeholder string for `index`.
    fn placeholder(index: usize) -> String { /* … */ }
}
```

Two associated constants are the only rendering decisions that change SQL
*semantics*, so they are named rather than hidden in method bodies:

* `SUPPORTS_ILIKE` — `ILIKE` is a PostgreSQL extension; SQLite only knows the
  case-sensitive `LIKE`. A dialect without `ILIKE` keeps the same
  case-insensitive semantics by folding both operands with `LOWER` when the
  predicate renders. `LOWER` folds ASCII letters only, which keeps the
  fallback independent of the database collation.
* `SUPPORTS_JSONB_ROW_AGG` — a dialect with native row aggregation renders a
  nested collection as `jsonb_agg(to_jsonb(…))`, keeping the clauses of the
  nested statement in charge of the produced document. A dialect without it
  assembles every object field by field with `json_object` and collects the
  objects with `json_group_array`. Both forms produce the same JSON document,
  so a client decodes one representation whichever engine produced it.

Everything else a dialect decides is spelling: `push_quoted_identifier`
renders `"users"` under PostgreSQL, and `push_placeholder` renders `$1` under
PostgreSQL and `?` under SQLite.

## Capabilities are traits, not flags

Each clause with engine-dependent availability gets its own marker trait —
`SupportsReturning`, `SupportsOnConflict`, `SupportsWindowFunctions`, and
eleven more — and the builder methods that would emit the clause are bounded
by that trait. A method that does not apply to a dialect cannot be called on
it, and `#[diagnostic::on_unimplemented]` turns the violation into a short
SQL-level message instead of a trait-solver cascade.

A dialect additionally carries an *engine version* type parameter, so a
capability is proven for the exact version range the application targets —
see [Versions](versions.md) and the catalogue in
[Capabilities](capabilities.md).

## In this chapter

* [Philosophy](philosophy.md) — why a dialect is a type and not a runtime flag.
* [Capabilities](capabilities.md) — the full marker catalogue and the `.build()` rules.
* [Versions](versions.md) — `Pg14`…`Pg16`, `Sqlite3_0`…`Sqlite3_38`, and capability thresholds.
* [Preludes](preludes.md) — binding a dialect without turbofish.
* [PostgreSQL](postgres/overview.md) — the PostgreSQL dialect, its extensions, and its drivers.
