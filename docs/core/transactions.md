# Transactions

RURQE offers two transactional surfaces, and the difference between them is what closes the transaction.

- A **closure-scoped block** lends the transaction to a closure. The adapter opens the block, nests a block opened inside it through a SQL savepoint, awaits the commit of a block that returned `Ok`, awaits the rollback of a block that returned `Err`, and returns the connection on either ending. The synchronous connections offer this form, and so does the asynchronous pool.
- A **transaction guard** lends the transaction itself to the caller. Opening one on the asynchronous connection hands back a handle that owns the transaction until its commit is awaited; the handle neither nests nor selects an isolation level, and dropping it without a commit asks the driver for a rollback the adapter cannot prove was delivered, so the connection is marked poisoned rather than reused.

Every guarantee a block carries therefore follows from what closes it: the adapter closes a closure-scoped block and awaits the statement that ends it, while a guard is closed by its own `Drop`, which cannot await anything. A guard that ends without a terminal call therefore proves nothing and leaves its connection refusing further work. A connection a block cannot prove clean is never committed either: the adapter closes such a block with a rollback and reports the poisoned-connection error instead of the value the closure returned.

Every refusal names what broke the transaction state, and keeps it. A poisoned connection reports the transaction-control statement that could not be completed, and — on the paths that trace the failure instead of returning it, namely the savepoint rollback of a nested block and the rollback a block issues once its closure returned — it also carries the driver failure behind the state and hands it back through `std::error::Error::source()`, so a caller reaches the variant, the SQLSTATE, and the cause chain the engine reported instead of parsing the message. A path that returns the driver failure to its own caller — a refused `BEGIN`, a refused `COMMIT`, a proof the pool could not obtain — reports the reason alone, because that caller already holds the failure itself.

## Asynchronous Transaction Guards

The guard is the asynchronous handle form: a transaction is opened on a live connection through `.transaction()`, which hands back a handle that owns the transaction for the whole block:

```rust,no_run
{{#rustdoc_include ../examples/transactions.rs:transaction}}
```

The handle implements the standard asynchronous terminal calls — `.fetch_all(&tx).await?`, `.fetch_one(&tx).await?`, `.execute(&tx).await?` — so queries and mutations run inside the active transaction. Committing is explicit: `commit().await?` persists the work and ends the transaction.

The handle neither selects an isolation level nor nests. It borrows the connection mutably for as long as it lives, so a second `.transaction()` call on the same connection is rejected by the borrow checker instead of opening a nested transaction, and no savepoint is opened for a block started through the handle. The closure-scoped forms of the same adapters offer both, and the rest of this chapter presents them.

### Failure mode of an abandoned entry

Opening a transaction is guarded on its own, because the statement that opens it and the work that follows it are two separate steps:

- A `BEGIN` the driver refuses is recorded on the connection, which is poisoned from that point on: the transaction state is unknown, so the connection refuses every later statement and every later transaction instead of assuming the session was left outside a transaction.
- The future of `.transaction()` is dropped before it completes, so no handle is handed back and the caller never learns whether the `BEGIN` reached the server. The adapter cannot prove the session is outside a transaction either, and poisons the connection the same way.

Both cases report the same poisoned-connection error as an abandoned handle, so a session that cannot be proven clean is never handed to later work.

### Failure mode of a dropped guard

Dropping the handle without committing requests a rollback, and that request is best effort rather than a guarantee:

- The rollback is issued from `Drop`, which cannot await a statement, so the driver neither awaits it nor reports its outcome.
- The driver does not make an in-flight statement cancel-safe. A future dropped in the middle of a statement can leave the connection waiting for a response, and the rollback the drop requested then never reaches the server.
- The work of the statements that completed stays pending in the open transaction until the connection is discarded.

Because the adapter cannot prove that the transaction was closed, a handle dropped without a terminal call marks its connection poisoned, exactly as an abandoned closure-scoped block does. `is_poisoned()` reports the state, and every further statement and every further transaction on that connection is refused with `PgError::ConnectionPoisoned` before it reaches the server, carrying the reason that produced the state. No later caller therefore joins the abandoned transaction, and the transaction itself is rolled back by the server when the session of the discarded connection ends, which is also why the mark is never cleared and the connection has to be discarded rather than reused.
A terminal call that is cancelled is the same ending one step later. The adapter takes the transaction out of the handle before it awaits the `COMMIT` or `ROLLBACK`, so a future dropped while that statement is in flight leaves the handle with nothing left to request, and the statement itself may never have reached the server. The adapter guards that window as well: a `commit()` or `rollback()` future dropped before it resolves poisons the connection exactly like a handle dropped without a terminal call, and every further statement and transaction on it is refused the same way.

A block that has to survive a cancelled future therefore closes itself through a closure-scoped form, which awaits the commit or the rollback before the result reaches the caller.

## Nested Transactions and Savepoints

This section documents the synchronous runtime form: the scoped transaction API of the synchronous adapters, where the closure returns a `Result` directly instead of a future. Calling `.transaction(...)` inside an active transaction scope transparently creates a nested transaction managed via SQL savepoints:

* An inner transaction boundary issues `SAVEPOINT savepoint_N`.
* If the inner closure succeeds with `Ok`, it executes `RELEASE SAVEPOINT savepoint_N`.
* If the inner closure returns `Err` or panics, it executes `ROLLBACK TO SAVEPOINT savepoint_N`.

The outer transaction remains open and can handle the error, allowing inner operations to fail and roll back without aborting the entire transaction.

When the savepoint rollback itself fails, the work of the enclosing scope is not abandoned to repair it. The connection is marked as failed for the enclosing scope: whether the pending work of the failed block still exists is unknown, so nothing may be committed over it and the failure is reported rather than absorbed.

- Further nested scopes on the same connection are refused with the driver's error for as long as the pending savepoint cannot be closed. A retry that heals the savepoint lets the next scope open normally.
- Every statement issued on the connection while it is marked failed is refused before it reaches the engine, carrying the failure that produced the state: a write would otherwise join the abandoned transaction and be discarded by the rollback that eventually closes it, while the caller observed it succeed. PostgreSQL answers the statement with `PgError::ConnectionPoisoned`; the SQLite adapter reports it as a driver conversion failure (`rusqlite::Error::ToSqlConversionFailure`) naming the recorded transaction-control failure and the recovery. The control statements of the guard itself are not refused, because the retried savepoint recovery and the rollback of a dropping scope are exactly the statements that end the state.
- The state is observable through `PgConnection::is_poisoned()` and `SqliteConnection::is_poisoned()`, so a caller holding the connection — a closure driving a pooled block, or an application holding an owned handle — detects that the abandoned transaction is still pending before it writes anything else. The mark clears once that transaction is closed for real, which is what the entry of the next block attempts.
- The enclosing transaction refuses to commit: committing it reports the driver error of the failed rollback, so the work that rollback was supposed to discard is never persisted.
- The enclosing scope's own rollback on drop is what finally closes the transaction. Once the enclosing scope is abandoned — by a returned error or by unwinding — the transaction is rolled back and the connection returns to a clean state for the next transaction.

The guarantee this establishes is one-directional: a failed nested scope is never recovered by rolling back the enclosing transaction. A nested block can lose its own work and nothing else, and the work of the enclosing scope is closed only by the enclosing scope itself.

```rust,no_run
{{#rustdoc_include ../examples/sync_transactions.rs:transaction}}
```

## Transaction Isolation Levels

This section documents the synchronous runtime form of an isolated transaction boundary. Transaction blocks can be configured with explicit SQL isolation levels:

* `ReadCommitted`: Prevents dirty reads (default in PostgreSQL).
* `RepeatableRead`: Prevents non-repeatable reads and phantom reads.
* `Serializable`: Highest isolation level, enforcing strict serializability.

Isolation levels are configured at the transaction boundary:

```rust,no_run
{{#rustdoc_include ../examples/sync_transaction_isolation.rs:isolation}}
```

The level reaches the statement that opens the outermost block — `BEGIN ISOLATION LEVEL <level>` — so every statement of the block observes it. A block opened while a transaction is already open nests inside a savepoint, and SQL defines no per-savepoint isolation, so the enclosing transaction keeps the level it was opened with and the requested one applies to the outermost block alone.

SQLite offers no isolation-level boundary at all: the engine serialises access through database-level locking, so the adapter's `BEGIN` ignores an isolation level by design.

## Raw Handle Escape Hatch

Synchronous adapters expose the underlying driver handle of the connection, so raw SQL can be executed directly and raw transaction control statements can be issued through it. That access is supported surface rather than a private detail: it is the escape hatch for behaviour the typed API does not model.

A transaction scope, however, owns the bookkeeping. A `BEGIN`, `COMMIT`, `ROLLBACK`, `SAVEPOINT`, `RELEASE SAVEPOINT`, or `ROLLBACK TO SAVEPOINT` issued through the raw handle inside a typed scope forfeits the guarantees of that scope — rollback on error or unwind, savepoint nesting, and commit on success — because the scope no longer observes the state of the transaction it believes it owns.

What such a desynchronisation costs is bounded, and deliberately so:

- A refused `ROLLBACK` — and likewise a refused savepoint rollback or a refused savepoint release — is classified as benign when the driver observes that no transaction is open. The block was already closed through the raw handle, so the connection is exactly as clean as the rollback wanted it to be, and the scope records it clean instead of marking it failed.
- A desynchronised scope therefore costs at most its own transaction call its driver error. It never leaves the adapter unable to open a new typed transaction, and a healthy connection is never wedged behind a failure it cannot clear.

The observation is driver-specific, because the engines disagree on a transaction-control statement issued with no transaction open:

- SQLite refuses `COMMIT` and `ROLLBACK` with an error, so the SQLite adapter classifies the refusal through the driver's autocommit check — the same observation the migration tooling uses to detect a script that left a transaction open. A refusal raised while no transaction is open is benign; a refusal that leaves a transaction open remains the driver's own error, is reported as such, and poisons the connection: the adapter then refuses every bare statement before it joins the abandoned transaction, and the record clears as soon as the transaction is closed for real — by the rollback of the enclosing block, or by the retry the next typed transaction entry performs before its block opens. A refused savepoint statement is classified the same way, so a nested block whose savepoint a raw handle already closed leaves the enclosing transaction poisoned for the same reason.
- PostgreSQL reports both statements in an idle session as warnings rather than errors, so no classification is needed there and the bookkeeping resynchronises through the server's own tolerance.

Where a leaked transaction is repaired depends on the surface, and both synchronous engines repair it the same way:

- The synchronous pool returns every checkout to an idle session, so a transaction the raw handle left open is closed before the next block opens and never reaches another caller.
- An owned connection returns the session to an idle state at the next typed transaction entry, before that block opens, in every build configuration. The PostgreSQL adapter sends a single `ROLLBACK`, which the server answers with a warning in a session that holds no transaction; the SQLite adapter reads the engine's autocommit flag and issues one only while a transaction is still open.
- A migration step proves the session the same way before it opens its block or runs an autocommit script.

The classification covers the rollback path only. A `COMMIT` that the raw handle desynchronised still costs its own transaction call its driver error, and the scope is closed by its rollback instead of being reported as successfully committed.
