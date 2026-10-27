# RURQE Philosophy

**RURQE** (**R**ust **U**niversal **R**elational **Q**uery **E**ngine) is a strictly typed SQL query builder, relational execution engine, and database-first code generation toolkit for Rust.

## The problem the project solves

The classic gap between application code and a relational database looks like this:

1. **The schema lives in two places.** DDL migrations describe one shape, application structs another, and the divergence surfaces at runtime (`column "..." does not exist`) instead of at build time.
2. **SQL is a string.** A string is not checked by the compiler: a typo in a column name, a reference to a table absent from `FROM`, a `LEFT JOIN` column read as a non-nullable value — every one of them is a runtime error.
3. **Types are lost at the boundary.** `SELECT` returns a tuple without names, `Option<T>` appears and disappears arbitrarily, and a `NULL` produced by an outer join arrives in a field declared as `T`.

RURQE formulates the answer as a single invariant: **if a query compiles, it is semantically correct for the target dialect and engine version**. An invalid query must not reach the database.

## Five principles

### 1. Code as SSOT (Single Source of Truth)

The schema is extracted from the live database catalog (`pg_catalog`, `information_schema`, the SQLite `PRAGMA` tables) instead of being rewritten by hand. Zero-sized table and column markers, DTO structs, and the catalog snapshot `schema.lock` with its `SCHEMA_HASH` checksum are generated from it.

The same principle applies to the book you are reading: every statement about a compiler guarantee or a runtime behavior is backed by a concrete location in the code base or by an existing test. Documentation that describes a capability absent from the code is a defect, not a wording issue.

### 2. Zero Database Rejection

No syntactically or semantically invalid query may reach the database engine. The error is caught either by the compiler (Tier 1) or by a deterministic check performed while the query is built (Tier 2) — see [Two-Tier SQL Correctness](two_tier.md).

### 3. Native-or-Nothing

A capability that cannot be expressed natively on the target engine is not emulated. `FOR UPDATE` on SQLite does not become "roughly the same thing": the method is simply absent from the dialect capability set (`SupportsRowLocking` is not implemented for `Sqlite`), and the code does not compile. `MERGE` follows the same policy: a synthetic emulation through `SELECT` + `INSERT/UPDATE` creates races and is forbidden.

### 4. Zero-Cost

The dialect is a zero-sized phantom type parameter (`SelectQuery<Postgres<Pg16>, ...>`), table and column markers are ZSTs, and query rendering is a single pass over the recorded AST into a `String` plus a parameter vector. No virtual calls, no reflection, no GC.

### 5. KISS, YAGNI, DRY

No "just in case" fields. Dead code is deleted immediately. Standard formats (TOML, JSON) and cryptography use mature crates rather than hand-written parsers; exceptions are stated explicitly and justified.

## What this gives in practice

| Promise | Mechanism in the code |
|---|---|
| A column typo is a compile error | `Column`, `TableInScope`, `ExprInScope` |
| A `NULL` from `LEFT JOIN` cannot be read as `T` | `TableNullabilityInScope`, `MakeNullable`, `ApplyNullability` |
| An `UPDATE` without `WHERE` cannot run accidentally | the `ExecutableMutation` typestate, `.allow_all()` |
| A construct the engine does not support does not compile | capability traits (`SupportsReturning`, `SupportsRowLocking`, ...) |
| An ambiguous `JOIN` is rejected before reaching the database | `BuildError::DuplicateRelation` in `.build()` |
| The schema does not diverge from the code | `cargo db-codegen`, `schema.lock`, `cargo db-verify` |

## Where to go next

- [The architectural tetrad](tetrad.md) — the four industrial ideas the project is built from.
- [Two-Tier SQL Correctness](two_tier.md) — where exactly the boundary between the compiler and `.build()` runs.
- [Quickstart in 5 minutes](quickstart.md) — the first working query.
- [Relational theory overview](../theory/overview.md) — why all of this is called relational algebra in types.
