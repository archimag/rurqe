# Introduction

RURQE is a strictly typed SQL query builder, relational execution engine, and database-first code generation toolkit for Rust. Correctness is established in two tiers: the type system rejects at compile time every relational violation it can express (relation scoping, projection typing, outer-join nullability, statement typestates, and dialect and version capabilities), and `.build()` rejects, before a statement is dispatched, the rules the type system cannot express, such as an unaliased duplicate relation or the `ORDER BY` alignment of `DISTINCT ON`. A query that passes both gates is a valid statement for the targeted dialect and engine version, provided its schema markers describe the live catalog.

## Guided Query Example

One chain carries the whole promise end to end: the entry point names the first relation, the join adds the second, the projection names the fields of the result, the compiler derives the type of each field from the clause that introduced its relation, and the statement the engine receives is exactly the one the chain assembled.

```rust,no_run
{{#rustdoc_include examples/joins.rs:outer}}
```

The result is a record with one field per projected column, so a row is read by name rather than by position:

```rust,no_run
{{#rustdoc_include examples/joins.rs:outer_rows}}
```

`row.user_id` is an `i64` and `row.user_name` a `String`, because the `FROM` relation is matched unconditionally; `row.order_total` is an `Option<f64>`, because a left join may leave the joined relation unmatched. Reading that field as an unwrapped `f64` fails to compile, and the query chain itself names no type at all.

The same chain renders into one statement, whose projection aliases carry the field names into the result:

```sql
SELECT "users"."id" AS "user_id", "users"."name" AS "user_name",
       "orders"."total" AS "order_total"
FROM "users" LEFT JOIN "orders" ON ("orders"."user_id" = "users"."id")
```

## Core Relational Problems

The boundary between application code and a relational database typically suffers from three fundamental issues:

1. **Schema drift:** Schema DDL lives in one place and application structs live in another. Divergence surfaces only at runtime when an executed query fails.
2. **Untyped SQL strings:** Raw strings bypass the compiler. Typos in column identifiers, omitted joins, or invalid nullability expectations remain invisible until execution.
3. **Loss of type guarantees:** Relational structures are flattened into tuples or weakly typed rows, losing provenance and nullability invariants.

RURQE resolves these challenges using compiler checks, zero-cost monomorphised abstractions, and deterministic catalog introspection. A query written through the typed DSL passes both validation gates before it is dispatched; a statement issued through the raw SQL escape hatches (`sql!`, `raw_sql!`) bypasses them by design, and its failures surface as typed execution errors at runtime.

## The Four Architectural Pillars

RURQE unifies four foundational design principles into a cohesive developer experience:

* **Query DSL Ergonomics:**
  * Pinned dialect preludes (`use rurqe::postgres::prelude::*;`) and value-based table markers (`select_from(User)`) eliminate generic parameters and turbofish syntax from application queries.
  * Named projection macros (`select!`, `cols!`, `dto!`) synthesise typed row records directly from column definitions, avoiding positional tuple indexing.
  * Editor tooling such as `rust-analyzer` provides full completion, type hints, and rename refactoring without requiring an active database server.
* **Relational Depth:**
  * The live database catalog serves as the single source of truth for schema definitions and generated column markers.
  * Comprehensive relational operations: multi-table joins, subqueries, common table expressions (`WITH`), window functions, grouping, `CASE WHEN` constructs, `LATERAL` joins, and set operations (`UNION`, `INTERSECT`, `EXCEPT`).
  * Mathematically sound nullability propagation: columns participating in `LEFT JOIN` or outer joins are automatically lifted to `Option<T>` at compile time.
* **Lifecycle Integrity:**
  * Dual lifecycle workflows: classic sequential migrations (`V...`, `R...` scripts with SHA-256 audit trails), or stage-based schema evolution, where a declarative stage holds the complete target DDL and an active migration tail advances databases that already hold data.
  * Instant cold-start initialisation: clean environments initialise directly from declarative schema DDL (`schema/`) in milliseconds.
  * Cryptographic checkpoint markers (`checkpoint.toml`) anchor migration tails and permit pruning historical migrations without rewriting baselines.
  * Continuous verification that the declarative stage and the migration tail produce the same catalog catches schema divergence in CI.
  * Both routes run from application code: the migration protocol and the catalog introspection of an engine live in its driver crate behind a `migrate` feature, so a binary applies the artifacts it bakes into itself over the connection it already holds — the command line is a convenience for the working loop, never a prerequisite.
* **Compile-Time Soundness and Zero-Cost Guarantees:**
  * Monomorphised builders compile down to static SQL string buffers and direct deserialisation passes with zero virtual calls or runtime overhead.
  * Dialect and version capability traits (`SupportsReturning`, `SupportsOnConflict`, `SupportsRowLocking`) enforce engine-specific syntax at compile time; unsupported methods do not exist on incompatible dialect targets.
  * Actionable compiler diagnostics via `#[diagnostic::on_unimplemented]` explain violated SQL rules directly rather than exposing internal trait cascades.

## Design Principles

* **The Schema Is the Single Source of Truth:** Table and column markers, DTO definitions, and catalog lock files (`schema.lock`) are derived from database catalog introspection rather than manually maintained.
* **Two-Tier Validation:** A query written through the typed DSL is validated at two gates before it is dispatched, and each gate owns the rules it can decide without the database.
  * **Tier 1 (compile time):** relation scope isolation, projection and `IN` subquery typing, outer-join nullability lifted to `Option<T>`, the typestates of a statement (a `SELECT` without a projection, an `UPDATE` without `SET`, an unfiltered `UPDATE` or `DELETE`), the projection rules of a grouped query, and dialect and version capability gates. A violation does not compile.
  * **Tier 2 (pre-flight in `.build()`):** the rules that need a negative assertion (a relation must be *absent* from the scope) or the alignment of a clause with another one written later in the chain. Among them: an unaliased duplicate relation, the leading `ORDER BY` prefix of `DISTINCT ON`, the `ORDER BY` coverage of `DISTINCT`, the ordering targets of a compound statement, and an aggregate projected alongside a bare column without `GROUP BY`. A violation returns `Err(BuildError)` instead of a statement, before any network I/O.
* **Native or Nothing:** Engine features that cannot be expressed natively are not simulated with synthetic runtime workarounds. If a database engine lacks a capability (such as row-level locking on SQLite), the builder method does not exist.
* **Zero Cost:** Monomorphised query chains and zero-sized markers compile directly to parameterised SQL strings and zero-allocation decoders without virtual dispatch or garbage collection.
* **Panic-Free Architecture:** Fallible operations consistently return typed errors (`Result<T, E>`), distinguishing build validation failures (`BuildError`), decoding mismatches (`RowDecodeError`), and backend failures (`ExecutionError`).

## Practical Guarantees

* Column typos and unjoined table references fail compilation immediately with messages naming the missing relation or column.
* Nullable values generated by `LEFT JOIN` or outer joins are represented as `Option<T>` and cannot be decoded into non-nullable scalars.
* Mutation statements (`UPDATE`, `DELETE`) require a filter predicate (`.where_(...)`) or an explicit `.allow_all()` call before they can be compiled or executed.
* Clauses unsupported by the active dialect or version do not exist on the query builder and are rejected at compile time.
* Ambiguous queries containing colliding unaliased table names are rejected prior to network transmission during `.build()`.
* Schema definitions and Rust structs remain synchronised via deterministic catalog hashing (`schema.lock`) and automated verification.
