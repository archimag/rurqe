# Introduction

RURQE is a strictly typed SQL query builder, relational execution engine, and database-first code generation toolkit for Rust. Its central promise: every query that compiles is a syntactically and semantically valid statement for the targeted database dialect and engine version, and an invalid query fails to compile.

## Guided Query Example

One chain carries the whole promise end to end: the builder declares the relations, the projection names the fields of the result, the compiler derives the type of each field from the clause that introduced its relation, and the statement the engine receives is exactly the one the chain assembled.

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

Relations can also be nested into collections within a single round trip, avoiding N+1 queries and manual client-side grouping:

```rust,no_run
{{#rustdoc_include examples/nested.rs:nested}}
```

The database aggregates child rows into a JSON structure, and the client deserializes the result directly into typed collections.

## Core Relational Problems

The boundary between application code and a relational database typically suffers from three fundamental issues:

1. **Schema drift:** Schema DDL lives in one place and application structs live in another. Divergence surfaces only at runtime when an executed query fails.
2. **Untyped SQL strings:** Raw strings bypass the compiler. Typos in column identifiers, omitted joins, or invalid nullability expectations remain invisible until execution.
3. **Loss of type guarantees:** Relational structures are flattened into tuples or weakly typed rows, losing provenance and nullability invariants.

RURQE resolves these challenges using compiler checks, zero-cost monomorphized abstractions, and deterministic catalog introspection. The database engine is never the first place where structural query defects are discovered.

## The Four Architectural Pillars

RURQE unifies four foundational design principles into a cohesive developer experience:

* **Query DSL Ergonomics:**
  * Pinned dialect preludes (`use rurqe::postgres::prelude::*;`) and value-based table markers (`select_from(User)`) eliminate generic parameters and turbofish syntax from application queries.
  * Named projection macros (`select!`, `cols!`, `dto!`) synthesize typed row records directly from column definitions, avoiding positional tuple indexing.
  * Editor tooling such as `rust-analyzer` provides full completion, type hints, and rename refactoring without requiring an active database server.
* **Relational Depth:**
  * The live database catalog serves as the single source of truth for schema definitions and generated column markers.
  * Comprehensive relational operations: multi-table joins, subqueries, common table expressions (`WITH`), window functions, grouping, `CASE WHEN` constructs, `LATERAL` joins, and set operations (`UNION`, `INTERSECT`, `EXCEPT`).
  * Mathematically sound nullability propagation: columns participating in `LEFT JOIN` or outer joins are automatically lifted to `Option<T>` at compile time.
* **Lifecycle Integrity:**
  * Dual lifecycle workflows: classic sequential migrations (`V...`, `R...` scripts with SHA-256 audit trails) alongside the declarative Stage + Tail model.
  * Instant cold-start initialization: clean environments initialize directly from declarative schema DDL (`schema/`) in milliseconds.
  * Cryptographic checkpoint markers (`checkpoint.toml`) anchor migration tails and permit pruning historical migrations without rewriting baselines.
  * Continuous verification of the Golden Equation catches schema divergence in CI.
* **Compile-Time Soundness and Zero-Cost Guarantees:**
  * Monomorphized builders compile down to static SQL string buffers and direct deserialization passes with zero virtual calls or runtime overhead.
  * Dialect and version capability traits (`SupportsReturning`, `SupportsOnConflict`, `SupportsRowLocking`) enforce engine-specific syntax at compile time; unsupported methods do not exist on incompatible dialect targets.
  * Actionable compiler diagnostics via `#[diagnostic::on_unimplemented]` explain violated SQL rules directly rather than exposing internal trait cascades.

## Design Principles

* **The Schema Is the Single Source of Truth:** Table and column markers, DTO definitions, and catalog lock files (`schema.lock`) are derived from database catalog introspection rather than manually maintained.
* **No Invalid Statement Reaches the Database:** Validation operates across two gates: the Rust compiler catches positive invariant violations (relation scoping, nullability lifting, typestates, capability checks), and client-side AST inspection during `.build()` catches negative invariants (duplicate unaliased relations) and cross-clause alignments.
* **Native or Nothing:** Engine features that cannot be expressed natively are not simulated with synthetic runtime workarounds. If a database engine lacks a capability (such as row-level locking on SQLite), the builder method does not exist.
* **Zero Cost:** Monomorphized query chains and zero-sized markers compile directly to parameterized SQL strings and zero-allocation decoders without virtual dispatch or garbage collection.
* **Panic-Free Architecture:** Fallible operations consistently return typed errors (`Result<T, E>`), distinguishing build validation failures (`BuildError`), decoding mismatches (`RowDecodeError`), and backend failures (`ExecutionError`).

## Practical Guarantees

* Column typos and unjoined table references fail compilation immediately with messages naming the missing relation or column.
* Nullable values generated by `LEFT JOIN` or outer joins are represented as `Option<T>` and cannot be decoded into non-nullable scalars.
* Mutation statements (`UPDATE`, `DELETE`) require a filter predicate (`.where_(...)`) or an explicit `.allow_all()` call before they can be compiled or executed.
* Clauses unsupported by the active dialect or version do not exist on the query builder and are rejected at compile time.
* Ambiguous queries containing colliding unaliased table names are rejected prior to network transmission during `.build()`.
* Schema definitions and Rust structs remain synchronized via deterministic catalog hashing (`schema.lock`) and automated verification.
