# Transactions

RURQE offers two transactional surfaces, and the difference between them is what closes the transaction.

- A **closure-scoped block** lends the transaction to a closure. The adapter opens the block, nests a block opened inside it through a SQL savepoint, awaits the commit of a block that returned `Ok`, awaits the rollback of a block that returned `Err`, and returns the connection on either ending. The synchronous connections and pools offer this form, and so do the asynchronous connection and pool.
- A **transaction guard** lends the transaction itself to the caller. Opening one on the asynchronous connection hands back a handle that owns the transaction until its commit is awaited; the handle neither nests nor selects an isolation level, and dropping it without a commit asks the driver for a rollback the adapter cannot prove was delivered, so the connection is marked poisoned rather than reused.

Every guarantee a block carries therefore follows from what closes it: the adapter closes a closure-scoped block and awaits the statement that ends it, while a guard is closed by its own `Drop`, which cannot await anything. A guard that ends without a terminal call therefore proves nothing and leaves its connection refusing further work.

## Asynchronous Transaction Guards

The guard is the asynchronous handle form: a transaction is opened on a live connection through `.transaction()`, which hands back a handle that owns the transaction for the whole block:

```rust,no_run
{{#rustdoc_include ../examples/transactions.rs:transaction}}
```

The handle implements the standard asynchronous terminal calls — `.fetch_all(&tx).await?`, `.fetch_one(&tx).await?`, `.execute(&tx).await?` — so queries and mutations run inside the active transaction. Committing is explicit: `commit().await?` persists the work and ends the transaction.

The handle neither selects an isolation level nor nests. It borrows the connection mutably for as long as it lives, so a second `.transaction()` call on the same connection is rejected by the borrow checker instead of opening a nested transaction, and no savepoint is opened for a block started through the handle. The closure-scoped forms of the same adapters offer both, and the rest of this chapter presents them.

### Failure mode of a dropped guard

Dropping the handle without committing requests a rollback, and that request is best effort rather than a guarantee:

- The rollback is issued from `Drop`, which cannot await a statement, so the driver neither awaits it nor reports its outcome.
- The driver does not make an in-flight statement cancel-safe. A future dropped in the middle of a statement can leave the connection waiting for a response, and the rollback the drop requested then never reaches the server.
- The work of the statements that completed stays pending in the open transaction until the connection is discarded.

Because the adapter cannot prove that the transaction was closed, a handle dropped without a terminal call marks its connection poisoned, exactly as an abandoned closure-scoped block does. `is_poisoned()` reports the state, and every further statement and every further transaction on that connection is refused with `PgError::ConnectionPoisoned` before it reaches the server, carrying the reason that produced the state. No later caller therefore joins the abandoned transaction, and the transaction itself is rolled back by the server when the session of the discarded connection ends, which is also why the mark is never cleared and the connection has to be discarded rather than reused.

A block that has to survive a cancelled future therefore closes itself through a closure-scoped form, which awaits the commit or the rollback before the result reaches the caller.

## Nested Transactions and Savepoints

This section documents the synchronous runtime form: the scoped transaction API of the synchronous adapters, where the closure returns a `Result` directly instead of a future. Calling `.transaction(...)` inside an active transaction scope transparently creates a nested transaction managed via SQL savepoints:

* An inner transaction boundary issues `SAVEPOINT savepoint_N`.
* If the inner closure succeeds with `Ok`, it executes `RELEASE SAVEPOINT savepoint_N`.
* If the inner closure returns `Err` or panics, it executes `ROLLBACK TO SAVEPOINT savepoint_N`.

The outer transaction remains open and can handle the error, allowing inner operations to fail and roll back without aborting the entire transaction.

When the savepoint rollback itself fails, the work of the enclosing scope is not abandoned to repair it. The connection is marked as failed for the enclosing scope: whether the pending work of the failed block still exists is unknown, so nothing may be committed over it and the failure is reported rather than absorbed.

- Further nested scopes on the same connection are refused with the driver's error for as long as the pending savepoint cannot be closed. A retry that heals the savepoint lets the next scope open normally.
- The enclosing transaction refuses to commit: committing it reports the driver error of the failed rollback, so the work that rollback was supposed to discard is never persisted.
- The enclosing scope's own rollback on drop is what finally closes the transaction. Once the enclosing scope is abandoned — by a returned error or by unwinding — the transaction is rolled back and the connection returns to a clean state for the next transaction.

The guarantee this establishes is one-directional: a failed nested scope is never recovered by rolling back the enclosing transaction. A nested block can lose its own work and nothing else, and the work of the enclosing scope is closed only by the enclosing scope itself.

```rust,no_run
{{#rustdoc_include ../examples/sync_transactions.rs:transaction}}
```

## Asynchronous Savepoint Nesting

The asynchronous closure form nests the same way, one awaited statement at a time. A block opened through the scope of an open transaction is a SQL savepoint of that transaction: the adapter issues `SAVEPOINT savepoint_N` before the nested closure runs, awaits `RELEASE SAVEPOINT savepoint_N` when the nested block returns `Ok`, and awaits `ROLLBACK TO SAVEPOINT savepoint_N` when it returns `Err`. The enclosing transaction stays open, so a nested block loses its own work and nothing else, and nesting works the same way one savepoint deeper.

```rust,no_run
{{#rustdoc_include ../examples/pool_transactions.rs:nesting}}
```

A rollback the driver refuses is traced instead of replacing the error of the block, and the connection is poisoned from that point on: further statements and further savepoints are refused with the poisoned-connection error, so the connection has to be discarded rather than reused, because the statements of the nested block stay pending inside the enclosing transaction.

## Transaction Isolation Levels

This section documents the synchronous runtime form of an isolated transaction boundary, followed by the asynchronous closure form. Transaction blocks can be configured with explicit SQL isolation levels:

* `ReadCommitted`: Prevents dirty reads (default in PostgreSQL).
* `RepeatableRead`: Prevents non-repeatable reads and phantom reads.
* `Serializable`: Highest isolation level, enforcing strict serializability.

Isolation levels are configured at the transaction boundary:

```rust,no_run
{{#rustdoc_include ../examples/sync_transaction_isolation.rs:isolation}}
```

The level reaches the statement that opens the outermost block — `BEGIN ISOLATION LEVEL <level>` — so every statement of the block observes it. A block opened while a transaction is already open nests inside a savepoint, and SQL defines no per-savepoint isolation, so the enclosing transaction keeps the level it was opened with and the requested one applies to the outermost block alone.

The asynchronous closure form takes the level the same way and awaits the boundary it opens:

```rust,no_run
{{#rustdoc_include ../examples/pool_transactions.rs:isolation}}
```

The adapter awaits the commit of a block that returned `Ok` and the rollback of a block that returned `Err`, so the isolation boundary is closed before the result reaches the caller. SQLite offers no isolation-level boundary at all: the engine serialises access through database-level locking, so the adapter's `BEGIN` ignores an isolation level by design.

## Pooled Transactions

A pool executes a single statement by checking a connection out, running it in autocommit, and returning the connection. Several statements become one atomic block through the closure form, which keeps one connection checked out for the whole duration of the block:

```rust,no_run
{{#rustdoc_include ../examples/pool_transactions.rs:transaction}}
```

- The outermost block of a pooled transaction runs between `BEGIN` and `COMMIT`; a block opened while a transaction is already open nests inside a `SAVEPOINT`.
- A block that returns `Err` has its statements rolled back before that error reaches the caller.
- A checkout failure surfaces as `PgError::Pool`, while a failure raised by `BEGIN`, by the block, or by `COMMIT` surfaces as `PgError`.
- A `ROLLBACK` the driver refuses supersedes the error of the block and surfaces as the poisoned-connection error `PgError::ConnectionPoisoned`, because the transaction state is unknown from that point on.
- A checkout whose block reached its end returns to the pool, whether the block committed or was rolled back, so several transactions of a shared pool run concurrently, each on its own connection.

A block that is cancelled or unwound never reaches the end of the call, so neither `COMMIT` nor `ROLLBACK` is awaited for it, and a `BEGIN`, `COMMIT`, or `ROLLBACK` the driver refuses leaves the transaction state unknown. Either ending destroys the checkout instead of returning it to the pool, so the connection never reaches another caller: dropping it closes its session, the server rolls the abandoned transaction back when that session ends, and the work of the abandoned block is discarded. The pool replaces the destroyed connection, so the next caller starts on a fresh session; a block that has to survive an abandoned future belongs in a task that finishes or reports its own cancellation.

### The synchronous pool

This section documents the synchronous runtime form. The synchronous pool hands its closure the checked-out connection, and the closure reports the driver error of that runtime. The pool reports a failure of the block as `SqlitePoolError::Query` and a failure to check a connection out as `SqlitePoolError::Pool`; the synchronous PostgreSQL pool reports the same pair as `PgPoolError::Query` and `PgPoolError::Pool`, over a closure that reports `PgError`. Either pool type converts from both failures it carries, so a checkout failure and a statement failure propagate with `?` into a function returning the pool error type, and a connection checked out of the pool reports that same type. The handle is cheap to clone, because every clone addresses the one underlying pool, and `pool()` hands back the wrapped pool for configuration or inspection performed outside the adapter. The three error types these surfaces report — `PgError`, `PgPoolError`, and `SqlitePoolError` — are open by design: their variants follow the adapters a build compiles, since the checkout failure of the asynchronous pool belongs to `PgError` only when that pool is compiled, so a match over any of them carries a wildcard arm and a variant added for a new adapter does not break the matches already compiled.

```rust,no_run
{{#rustdoc_include ../examples/sync_pool_transactions.rs:transaction}}
```

The block commits when the closure returns `Ok` and rolls back when the closure returns `Err` or unwinds, exactly as the owned connection does, and the checkout returns to the pool on every one of those paths.

The synchronous PostgreSQL pool returns every connection to an idle session before handing it out: the reset is one round trip carrying `BEGIN` followed by `ROLLBACK`, and it leaves no trace on a session that was already idle. A transaction an earlier checkout left open — one issued through the raw handle, or one whose control statement the driver refused — is therefore closed before the next block opens, so neither a transaction block nor a single statement issued through the pool joins an abandoned transaction. A session the reset cannot bring back to a known state is never handed out: the checkout fails with the abandoned-transaction refusal `PgError::AbandonedTransaction`, carried by `PgPoolError::Query`, instead of running a block inside the abandoned transaction.

A block opened inside a pooled transaction nests the same way:

```rust,no_run
{{#rustdoc_include ../examples/sync_pool_transactions.rs:nesting}}
```

Returning `Ok` releases the savepoint of the nested block; returning `Err` or unwinding rolls it back, and the enclosing block stays open to commit its own work.

## Raw Handle Escape Hatch

Synchronous adapters expose the underlying driver handle of the connection, so raw SQL can be executed directly and raw transaction control statements can be issued through it. That access is supported surface rather than a private detail: it is the escape hatch for behaviour the typed API does not model.

A transaction scope, however, owns the bookkeeping. A `BEGIN`, `COMMIT`, `ROLLBACK`, `SAVEPOINT`, `RELEASE SAVEPOINT`, or `ROLLBACK TO SAVEPOINT` issued through the raw handle inside a typed scope forfeits the guarantees of that scope — rollback on error or unwind, savepoint nesting, and commit on success — because the scope no longer observes the state of the transaction it believes it owns.

What such a desynchronisation costs is bounded, and deliberately so:

- A refused `ROLLBACK` — and likewise a refused savepoint rollback or a refused savepoint release — is classified as benign when the driver observes that no transaction is open. The block was already closed through the raw handle, so the connection is exactly as clean as the rollback wanted it to be, and the scope records it clean instead of marking it failed.
- A desynchronised scope therefore costs at most its own transaction call its driver error. It never leaves the adapter unable to open a new typed transaction, and a healthy connection is never wedged behind a failure it cannot clear.

The observation is driver-specific, because the engines disagree on a transaction-control statement issued with no transaction open:

- SQLite refuses `COMMIT` and `ROLLBACK` with an error, so the SQLite adapter classifies the refusal through the driver's autocommit check — the same observation the migration tooling uses to detect a script that left a transaction open. A refusal raised while no transaction is open is benign; a refusal that leaves a transaction open remains the driver's own error and is reported as such.
- PostgreSQL reports both statements in an idle session as warnings rather than errors, so no classification is needed there and the bookkeeping resynchronises through the server's own tolerance. Where a leaked transaction is repaired depends on the surface: the synchronous pool returns every checkout to an idle session, so a transaction the raw handle left open is closed before the next block opens and never reaches another caller; an owned connection runs the next block inside the leaked transaction in a release build, while a debug build returns the session to an idle state at the next typed transaction entry, before that block opens.

The classification covers the rollback path only. A `COMMIT` that the raw handle desynchronised still costs its own transaction call its driver error, and the scope is closed by its rollback instead of being reported as successfully committed.
