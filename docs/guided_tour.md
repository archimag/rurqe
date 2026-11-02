# Guided Tour

This tour walks the practical surface of the library end to end: the schema markers a query addresses, a typed read, a row that crosses a function boundary, a statement composed conditionally at runtime, the row mutations, a join with its automatic nullability, an aggregation, a nested collection, a transaction, the dialect release a statement is pinned to, and the gates a statement passes before it reaches an engine. Every Rust block below is a fragment of the compiled example crate, so the code shown is the code the workspace builds.

## Schema markers

A table is a zero-sized marker type, and each of its columns is a marker that carries the Rust type of the column. The markers are values, so a query names a relation or a column directly and the chain carries no generic annotation.

```rust,no_run
{{#rustdoc_include examples/quickstart.rs:schema}}
```

The canonical source of these declarations is the live database catalog: `cargo rurqe codegen` introspects the configured database and writes the marker module together with the catalog snapshot beside it. A hand-written `table!` declaration stays available for isolated unit tests and prototypes, where no database is reachable while the crate is built.

## Typed reads

A read names its relation, projects the columns, and filters the rows. Only the terminal call touches a connection, so the chain itself is assembled without a network round trip.

```rust,no_run
{{#rustdoc_include examples/quickstart.rs:query}}
```

The projection decides the shape of the row: every projected item becomes a field, named after its column marker unless the name is written out.

```rust,no_run
{{#rustdoc_include examples/quickstart.rs:named_rows}}
```

```sql
SELECT "id" AS "user_id", "name" AS "contact_email", "email" AS "user_email"
FROM "users"
WHERE ("email" = $1)
ORDER BY "id" ASC
```

A fetched row is read by those names, and the order of the projected items never becomes part of the reading.

## A row that crosses a function boundary

`select!` and `cols!` produce a record that is anonymous and local to the chain that carries them. A row returned from a function is declared once with `dto!`, which reads every field type from the schema instead of restating it:

```rust,no_run
{{#rustdoc_include examples/quickstart.rs:dto}}
```

The declaration implements the row-decoding contract, so the values of a row become the struct wherever the row is used:

```rust,no_run
{{#rustdoc_include examples/quickstart.rs:dto_row}}
```

## Conditional composition

A handler receives its filter flags at runtime. `.when(...)` and `.if_some(...)` add a clause only when the flag is set or the value is present, and the statement keeps its type either way:

```rust,no_run
{{#rustdoc_include examples/quickstart.rs:conditional}}
```

A false flag and a `None` add nothing at all, so one chain serves every combination of the runtime inputs without a variant of the query per branch.

## Mutations

`INSERT` names its target and assigns one value per column, and every assignment checks the type of its column while the chain is written:

```rust,no_run
{{#rustdoc_include examples/crud.rs:insert}}
```

`.returning(...)` is capability-gated: it exists only for a dialect that supports the clause and it projects the columns of the written rows.

`UPDATE` requires a filter before it can execute, so a statement that rewrites every row has to say so with an explicit `.allow_all()`:

```rust,no_run
{{#rustdoc_include examples/crud.rs:update}}
```

`DELETE` carries the same requirement:

```rust,no_run
{{#rustdoc_include examples/crud.rs:delete}}
```

Conflict handling is expressed natively, so an insert that meets an existing row follows the action the statement declares:

```rust,no_run
{{#rustdoc_include examples/crud.rs:upsert}}
```

## Joins and automatic nullability

An outer join introduces a relation that may leave a row unmatched, and the type of every column read through that relation follows from the clause that introduced it, before the projection is written:

```rust,no_run
{{#rustdoc_include examples/joins.rs:outer}}
```

```sql
SELECT "users"."id" AS "user_id", "users"."name" AS "user_name",
       "orders"."total" AS "order_total"
FROM "users" LEFT JOIN "orders" ON ("orders"."user_id" = "users"."id")
```

The stored column is an `f64` and the projected field is an `Option<f64>`, because the join decides the nullability of the relation rather than the projection deciding it:

```rust,no_run
{{#rustdoc_include examples/joins.rs:outer_rows}}
```

Reading that field as an unwrapped scalar fails to compile, so a missing row can never be decoded into a non-nullable field.

## Aggregation

`.group_by(...)` moves the query into a grouped statement. From that point the projection accepts grouped columns, aggregates, constants, and window expressions, and a stored column outside the grouping is refused at the call site:

```rust,no_run
{{#rustdoc_include examples/aggregates.rs:group_by}}
```

`count_all()` is an aggregate, so it stays projectable after the grouping, and the same expression filters the groups in `.having(...)`.

## Nested collections

A one-to-many relation becomes a typed collection in a single round trip. The child query is nested into the projection, the engine aggregates its rows into one JSON document per parent, and the client decodes that document into the row type of the child projection, so there is one statement rather than one per parent:

```rust,no_run
{{#rustdoc_include examples/nested.rs:nested}}
```

The nested statement carries its own scope, so a column of the enclosing query enters the child predicate through `.without_scope()`, which marks the expression as resolved by the enclosing statement.

## Transactions

A transaction is a guard over a live handle. It commits only when the handle commits, and it rolls the transaction back when it is dropped without one, so a returned error or a cancelled future leaves no partial work behind:

```rust,no_run
{{#rustdoc_include examples/transactions.rs:transaction}}
```

The guard hands the standard terminal calls a transactional scope, so a statement executed against it participates in the transaction the guard opened.

## Pinning a dialect

A dialect prelude binds identifier quoting, parameter placeholders, and the capability set of the target engine, so a query names no dialect type of its own:

```rust,no_run
{{#rustdoc_include examples/dialects.rs:postgres}}
```

The same chain against SQLite renders positional `?` placeholders instead of the numbered `$1` form:

```rust,no_run
{{#rustdoc_include examples/dialects.rs:sqlite}}
```

A version-pinned module carries the engine release as well, and a clause that release does not support does not exist on the builder at all:

```rust,no_run
{{#rustdoc_include examples/dialects.rs:version}}
```

## Validation before execution

The compiler settles the positive invariants: the scope a statement may reference, the nullability of a projected field, the typestate of a mutation, and the capabilities of a dialect. A rule that needs a negative assertion, or the alignment of one clause with another, is settled deterministically by `.build()` — the same step every terminal call performs before it sends anything:

```rust,no_run
{{#rustdoc_include examples/quickstart.rs:build_refusal}}
```

`DISTINCT ON` deduplicates the rows on one expression, so the leading `ORDER BY` criterion has to name the same expression. The builder refuses the statement above and reports the position where the two clauses disagree, instead of leaving the rejection to the engine.

## Errors

Every fallible step returns a typed error, so a failure keeps the classification that produced it:

- `BuildError` reports a statement that violates the structural rules of the SQL AST: a relation addressed twice under one name, a `DISTINCT ON` prefix that disagrees with the ordering, a raw template whose placeholders and parameters differ in number, or a projection that records nothing.
- `RowDecodeError` reports a returned value that cannot be decoded into the type the row declared.
- `ExecutionError` reports a terminal call that failed, and it carries the driver error of the dialect together with its classification helpers (`is_not_found`, `is_multiple_rows`, `is_row_decode`, `as_backend`).
