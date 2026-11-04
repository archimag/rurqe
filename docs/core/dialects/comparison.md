# Comparing PostgreSQL and SQLite

RURQE targets two engines with a single builder: PostgreSQL as a client-server database and SQLite as an embedded one. The dialect is a compile-time parameter of a statement — it decides how identifiers and parameters are spelled and which clauses the pinned release of the engine can parse — and nothing about it survives into execution. The same chain therefore composes against either engine, and only the terminal call names the destination.

This chapter compares the two engines where a choice between them is made: the deployment model each assumes, the way each serialises concurrent work, what a transaction promises, what the two type systems can declare, and which expressions only one of them defines. A clause the pinned release does not provide has no builder method on that dialect, so an unsupported statement is rejected by the compiler instead of by the engine.

## Deployment Model

- **Concurrency.** PostgreSQL isolates concurrent writers per row: a transaction claims only the rows it modifies, so other transactions keep writing the rest of the table. SQLite serialises writers through the database file: a write takes the whole file, and no statement claims an individual row.
- **Runtime.** PostgreSQL ships a blocking adapter and an asynchronous one, so one statement executes on a worker thread or on an asynchronous executor. SQLite ships a blocking adapter only, because an embedded statement completes inside the calling process.
- **Lifecycle.** A PostgreSQL database is a long-lived deployment target that exists before the application connects and outlives it. A SQLite database is created on demand as a file, or as a private in-memory image that disappears together with the connection.

The lifecycle difference belongs to the deployment rather than to a statement: a file-backed database hands the adapter a connection opened on a path, and a test suite or a local deployment opens a private in-memory image instead. The same builder chain executes against either one, because the connection is chosen at the terminal call.

## Concurrency and Locking

PostgreSQL runs readers and writers at the same time: a reader works on a consistent snapshot without blocking a writer, and two writers do not block each other unless they modify the same row. Inside a transaction a statement may claim the rows it selects, and the claim lasts until the transaction ends:

- `for_update()` renders `FOR UPDATE`, which keeps other transactions from modifying the selected rows, while `for_share()` renders `FOR SHARE`, which lets them read the rows and share the lock.
- `nowait()` and `skip_locked()` decide what happens when another transaction already holds a conflicting claim: the statement either fails immediately or passes over the rows it cannot claim.

That is how one query hands a row of a queue to exactly one consumer.

SQLite writes one transaction at a time: a write claims the database file with `BEGIN IMMEDIATE`, and the claim is released when the transaction ends. There is no per-row claim, so the locking methods have no builder equivalent on that dialect.

Beyond rows, a PostgreSQL session coordinates with other sessions through advisory locks: such a lock belongs to the session that took it rather than to a row or a table, and the migration runner holds one for the length of a run so two runners cannot interleave. SQLite has no such facility; the open write transaction is what holds a run's place, so the migration runner keeps that transaction open for the whole run instead.

## Transactions

`transaction(|tx| …)` opens a closure-scoped block on a blocking connection and on either pool, and the adapter that opened it closes it: the commit of a closure that returned `Ok` and the rollback of a closure that returned `Err` are awaited, so a failed block leaves no partial work behind. A block opened inside another one is carried by a savepoint rather than by a second transaction, so an inner failure rolls back only its own statements. The asynchronous connection opens the closure-scoped form through `transaction_with_isolation`, which takes the isolation level as its first argument.

The asynchronous connection also hands the transaction itself to the caller, and the contract of that handle differs. `conn.transaction().await?` returns a handle that commits only when `commit()` is awaited; it neither nests nor selects an isolation level, so a second call on the same connection is rejected by the borrow checker instead of opening a savepoint. Dropping the handle without a terminal call requests a rollback that its `Drop` can neither await nor observe, so the statements that completed stay pending in the open transaction, and the adapter marks the connection poisoned: every later statement and every later transaction on it is refused with `PgError::ConnectionPoisoned` instead of joining the abandoned transaction, and the connection is discarded rather than reused. A `commit()` or `rollback()` future dropped before its statement resolves leaves the same state behind. A block that has to survive a cancelled future therefore closes itself through the closure-scoped form.

Isolation is a property of the server: `transaction_with_isolation` opens a PostgreSQL block at `ReadCommitted`, `RepeatableRead`, or `Serializable`. A SQLite block takes no level, because the engine serialises access through database-level locking and a level would have no `BEGIN` clause to map onto.

Some statements refuse a transaction block at all: `VACUUM` and a `PRAGMA` that rewrites the database file are the ones a SQLite migration meets, and a concurrent index build is the one a PostgreSQL migration meets. A migration script that declares itself non-transactional is executed in autocommit, and the run records the attempt before the script starts, so an interrupted script stays visible instead of unrecorded.

## Data Types

Both introspectors describe a catalog in the same vocabulary: boolean, integer, floating-point, numeric, text, bytes, UUID, JSON, timestamp, date, enum, and array kinds. A generated schema module therefore has the same shape on either engine, and the Rust type of a column marker follows from the kind the catalog reports.

What differs is what an engine can declare, and the difference survives into the generated module:

- PostgreSQL carries a native type for a UUID, a native document type (`json` and `jsonb`), an array type (`T[]`), and an enum created with `CREATE TYPE … AS ENUM`. Each of them is a column type the server validates.
- SQLite has storage classes and type affinity. The introspector maps a declared type onto the shared kinds, recovers an enum from a `CHECK (col IN (...))` constraint, and reads a document as JSON text rather than as a type of its own. The engine has no array column type, so a catalog it produces describes no array column.
- A document operation follows the same split: `@>` and `?` are defined for `jsonb` alone, while `->` and `->>` read either kind, so a column states its kind through `PgJsonb` or `PgJson` and only the operators that kind supports are offered; SQLite evaluates a document through its JSON functions, which is also how a nested collection is assembled there.

## Engine Extensions

The PostgreSQL adapter carries the expressions only that engine defines: the regular-expression predicates (`~`, `~*`, `!~`, `!~*`), the document operators, the array containment and overlap operators, and full-text search. They are not portable sugar over a shared surface — each of them renders an operator the engine itself implements, and a pattern or a document binds as an ordinary parameter.

SQLite defines none of them, and its adapter adds no equivalent expression surface of its own. Its differences from PostgreSQL are rendering-level: a case-insensitive predicate folds both operands through `LOWER`, and a nested collection is aggregated from the JSON functions.

A statement outside the modelled surface is issued as raw SQL — through `sql!` or `raw_sql!`, or through the driver handle — and behaves exactly as the engine defines it.

## Statement Rendering

Both dialects quote an identifier with double quotes and double every embedded quote, so a reserved word or a mixed-case name needs no attention from the caller. The bind parameter is where the two part: PostgreSQL numbers its parameters (`$1`, `$2`, …), while SQLite writes an anonymous `?` for each one. The parameter list is ordered either way, so the difference stays inside the statement text — and a nested statement continues the list of the statement that projects it, so a filter written inside a nested collection stays independent of one written outside it.

```rust,no_run
{{#rustdoc_include ../../examples/dialects.rs:postgres}}
```

```sql
SELECT "id" AS "user_id" FROM "users" WHERE ("id" = $1)
```

```rust,no_run
{{#rustdoc_include ../../examples/dialects.rs:sqlite}}
```

```sql
SELECT "id" AS "user_id" FROM "users" WHERE ("id" = ?)
```

The remaining rendering rules follow the same split:

| Rendering rule | PostgreSQL | SQLite |
|---|---|---|
| Identifier quoting | `"users"` | `"users"` |
| Bind placeholder | `$1`, `$2`, … | `?` |
| Case-insensitive matching | `ILIKE` | `LIKE` over `LOWER`-folded operands |
| Nested collection document | `jsonb_agg(to_jsonb(...))` | `json_group_array(json_object(...))` |

Both chains above assemble one statement over the same clauses, and the dialect shows in the text alone: the entry point comes from the module that binds the engine, and every builder call after it is identical.

## Choosing an Engine

Choose PostgreSQL when the deployment needs several processes writing at once, a claim on individual rows for a queue, an explicit isolation level, a native document or array column, or a predicate the server evaluates itself — a regular expression or a full-text query. The server is a deployment target of its own: it is provisioned, backed up, and reached over a connection or a pool.

Choose SQLite when one process owns the data: a command-line tool, a background worker, an embedded application, or a test suite that opens an in-memory database per case. The database is a file or a private in-memory image, the deployment ships no server, and the engine serialises writers through that file.

The choice does not shape the code base. A statement is pinned to one dialect at the entry point that builds it, and a second engine is reached by composing the same chain under another module; a clause the target does not implement is rejected while the statement is built — by the compiler where the dialect lacks the capability, and by `.build()` where the rule needs the assembled statement to be checked.
