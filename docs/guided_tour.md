# Guided Tour

This tour follows one application from the declaration of its schema to its transactional writes. The application keeps user accounts and their orders: the schema is generated from the database catalog, the read path grows from a single lookup into joins, aggregates, and nested collections, the write path ends with a transaction that records several rows atomically, and the closing sections pin the target engine and show the gates a statement passes before it reaches one. Every section continues the case the previous one introduced, and every Rust block is a fragment of the compiled example crate, so the code shown is the code the workspace builds.

## Schema markers

The tour starts at the model. A table is a zero-sized marker type, and each of its columns is a marker that carries the Rust type of the column. The markers are values, so a query names a relation or a column directly and the chain carries no generic annotation.

```rust,no_run
{{#rustdoc_include examples/quickstart.rs:schema}}
```

The module above has the shape the generator writes: the digest of the catalog snapshot it was generated from, one marker per table, and one marker per column. An application imports that module and addresses the markers it declares; the example files of this tour declare the tables they read in the same form, because an example target is a single file. A hand-written `table!` declaration stays available where no database is reachable while the crate is built, and the markers of both forms are addressed identically.

## Reading rows

The first endpoint fetches one account by identifier. The chain names the relation, projects the columns the endpoint returns, and filters the rows; only the terminal call touches the connection.

```rust,no_run
{{#rustdoc_include examples/quickstart.rs:query}}
```

The projection decides the shape of the row: every projected item becomes a field of the record the terminal call returns.

## Named projections

The account report needs the same row in the vocabulary of the caller, so the projection writes the name of every field out. The alias travels into the statement, and the fetched row is read by the names that were written:

```rust,no_run
{{#rustdoc_include examples/quickstart.rs:named_rows}}
```

```sql
SELECT "id" AS "user_id", "name" AS "contact_email", "email" AS "user_email"
FROM "users"
WHERE ("email" = $1)
ORDER BY "id" ASC
```

The order of the projected items never becomes part of the reading, so a field keeps its name wherever the row is used.

## Reusable row records (`dto!`)

The record of `select!` and `cols!` is anonymous and local to the chain that carries it. A row that crosses a function boundary is declared once with `dto!`, which reads every field type from the schema instead of restating it:

```rust,no_run
{{#rustdoc_include examples/quickstart.rs:dto}}
```

The declaration implements the row-decoding contract, so the values of a row become the struct wherever the row is used:

```rust,no_run
{{#rustdoc_include examples/quickstart.rs:dto_row}}
```

## Conditional composition

The list endpoint receives its filters at runtime: a flag that enables one predicate, and an optional search term. `.when(...)` and `.if_some(...)` add a clause only when the flag is set or the value is present, and the statement keeps its type either way:

```rust,no_run
{{#rustdoc_include examples/quickstart.rs:conditional}}
```

A false flag and a `None` add nothing at all, so one chain serves every combination of the runtime inputs without a variant of the query per branch.

## Outer joins and nullability

The next screen lists what each account spent. The statement reads the accounts of `users` and the orders of `orders` together, and an account without a single order must still appear, so the second relation is introduced by a left join:

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

The dashboard aggregates instead of listing: it groups the rows of the joined statement by account and counts the orders of each group.

```rust,no_run
{{#rustdoc_include examples/aggregates.rs:group_by}}
```

`.group_by(...)` moves the query into a grouped statement. From that point the projection accepts grouped columns, aggregates, constants, and window expressions, and a stored column outside the grouping is refused at the call site. `count_all()` is an aggregate, so it stays projectable after the grouping, and the same expression filters the groups in `.having(...)`.

## Nested collections

The order history screen returns every account together with its orders, in one round trip rather than one statement per account. The child query is nested into the projection, the engine aggregates its rows into one JSON document per parent, and the client decodes that document into the row type of the child projection:

```rust,no_run
{{#rustdoc_include examples/nested.rs:nested}}
```

The nested statement carries its own scope, so a column of the enclosing query enters the child predicate through `outer(...)`, which marks the expression as resolved by the enclosing statement.

## Row mutations

Registration writes the account row. The insert names its target, assigns one value per column, and every assignment checks the type of its column while the chain is written:

```rust,no_run
{{#rustdoc_include examples/crud.rs:insert}}
```

`.returning(...)` is capability-gated: it exists only for a dialect that supports the clause and it projects the columns of the written rows.

An update requires a filter before it can execute, so a statement that rewrites every row has to say so with an explicit `.allow_all()`:

```rust,no_run
{{#rustdoc_include examples/crud.rs:update}}
```

A delete carries the same requirement:

```rust,no_run
{{#rustdoc_include examples/crud.rs:delete}}
```

Conflict handling is expressed natively, so an insert that meets an existing row follows the action the statement declares:

```rust,no_run
{{#rustdoc_include examples/crud.rs:upsert}}
```

## Transactions

Opening the two balances of a new account pair writes two rows that must succeed or fail together. A transaction is a guard over a live handle: it commits only when the handle commits and rolls the transaction back when it is dropped without one, so a returned error or a cancelled future leaves no partial work behind:

```rust,no_run
{{#rustdoc_include examples/transactions.rs:transaction}}
```

The guard hands the standard terminal calls a transactional scope, so a statement executed against it participates in the transaction the guard opened.

## Dialect pinning

A dialect prelude binds identifier quoting, parameter placeholders, and the capability set of the target engine, so a query names no dialect type of its own:

```rust,no_run
{{#rustdoc_include examples/dialects.rs:postgres}}
```

The same chain against an embedded deployment renders positional `?` placeholders instead of the numbered `$1` form:

```rust,no_run
{{#rustdoc_include examples/dialects.rs:sqlite}}
```

A version-pinned module carries the engine release as well, and a clause that release does not support does not exist on the builder at all:

```rust,no_run
{{#rustdoc_include examples/dialects.rs:version}}
```

## Pre-flight validation

The compiler settles the positive invariants: the scope a statement may reference, the nullability of a projected field, the typestate of a mutation, and the capabilities of a dialect. A rule that needs a negative assertion, or the alignment of one clause with another, is settled deterministically by `.build()` — the same step every terminal call performs before it sends anything:

```rust,no_run
{{#rustdoc_include examples/quickstart.rs:build_refusal}}
```

`DISTINCT ON` deduplicates the rows on one expression, so the leading `ORDER BY` criterion has to name the same expression. The builder refuses the statement above and reports the position where the two clauses disagree, instead of leaving the rejection to the engine.

## Typed errors

Every fallible step returns a typed error, so a failure keeps the classification that produced it:

- `BuildError` reports a statement that violates the structural rules of the SQL AST: a relation addressed twice under one name, a `DISTINCT ON` prefix that disagrees with the ordering, a raw template whose placeholders and parameters differ in number, or a projection that records nothing.
- `RowDecodeError` reports a returned value that cannot be decoded into the type the row declared.
- `ExecutionError` reports a terminal call that failed, and it carries the driver error of the dialect together with its classification helpers (`is_not_found`, `is_multiple_rows`, `is_row_decode`, `as_backend`).
