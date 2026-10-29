# Everyday queries: the portable core

This part is the tour of the everyday surface — the half of the DSL that does not depend on a specific engine: projections, filters, ordering, grouping, windows, joins, aliases, subqueries, CTEs, set operations, DML, nested collections, and the transaction a group of them runs in.

Three rules hold across every chapter:

| Rule | What it looks like in your code |
|---|---|
| **No turbofish** | `select_from(User)`, `insert_into(User)`, `update(User)`, `delete_from(User)` — one import pins the dialect, and the table is passed as a value |
| **Named fields** | `select!` / `cols!` synthesize a record with the field names of the projection, so a row is read as `row.user_id`, never `row.0` |
| **Reusable shapes** | `dto!` declares a row shape once, with the field types read from the schema |

The fragments of this part read two tables — `users` and `orders` — declared as zero-sized markers, one marker per column. In a real project the markers are not written by hand: the code generator reads the live catalog and emits the same types together with the catalog snapshot that CI verifies. Every fragment below is included from the example crate the workspace compiles, so a chapter cannot drift from the code it documents.

## A query you would actually write

```rust,no_run
{{#rustdoc_include ../examples/quickstart.rs:query}}
```

No column type is repeated, no dialect is named, no tuple is indexed. The projection is written once and the produced row carries one field per projected marker, so the reading side addresses `row.user_id` and never `row.0`.

## The same query, assembled at runtime

```rust,no_run
{{#rustdoc_include ../examples/quickstart.rs:conditional}}
```

A false flag or an absent value adds no clause, and the query keeps its type, so nothing downstream has to be re-typed. The clauses that change the type of the statement — a projection, a join, a lock — stay outside those branches.

## Aggregations keep the same ergonomics

```rust,no_run
{{#rustdoc_include ../examples/aggregates.rs:group_by}}
```

`count_all()` is an ordinary value: it compares in `HAVING` like any other expression, and the grouped query accepts exactly the columns it grouped by plus the aggregates — anything else is refused at the call site instead of by the engine.

## One query instead of N+1

A one-to-many relation needs neither a second statement nor a client-side fold: the engine aggregates the child rows into a JSON document, and the client decodes it into a typed collection.

```rust,no_run
{{#rustdoc_include ../examples/nested.rs:nested}}
```

The child rows are aggregated by the engine, so the ordering, the filter, and the row limit of the nested statement decide the contents of the collection.

## A group of writes is one transaction

Atomicity is written into the chain rather than configured afterwards: the block commits when its closure returns `Ok` and rolls every statement back when it does not, while a block opened inside it becomes a savepoint instead of a second transaction.

```rust,no_run
{{#rustdoc_include ../examples/transactions.rs:transaction}}
```

The block runs on the synchronous connection, which is what a migration runner, a CLI tool, or an embedded deployment opens; the shape of the block is the same over an asynchronous handle.

## The pinned version decides which clauses exist

One import also fixes the engine release, and that release alone decides which clauses the compiler accepts: the builder chain never changes, only the method set does.

```rust,no_run
{{#rustdoc_include ../examples/dialects.rs:version}}
```

The statement is the one any other release composes as well. Pinning an older release does not defer the failure to runtime — it removes the clauses that release does not support from the method set, so the call cannot be written at all.

## What the chapters rely on

| Crate | What these chapters use from it |
|---|---|
| `rurqe-core` | the query builders, the expression model, the pre-flight validation |
| `rurqe-macros` | `select!`, `cols!`, `dto!`, `sql!`, `#[derive(SqlType)]` |
| `rurqe` | the entry points, the prelude, and the version-pinned dialect modules |

Engine specifics — capability gates, versions, dialect extensions — are bound by the dialect prelude a statement imports.

## How the chapters are written

Every chapter of this part follows one scheme: the task, the code, the SQL it produces, and the mistake the compiler refuses.
