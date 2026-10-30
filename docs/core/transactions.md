# Transactions

RURQE provides RAII-managed transactions with explicit commit on success, automatic rollback on drop, and transparent nesting via SQL savepoints on the synchronous adapters.

## RAII Transaction Scopes

The primary form is asynchronous: a transaction is opened on a live connection through `.transaction()`, which hands back a handle that owns the transaction for the whole block:

```rust,no_run
{{#rustdoc_include ../examples/transactions.rs:transaction}}
```

The handle implements the standard asynchronous terminal calls — `.fetch_all(&tx).await?`, `.fetch_one(&tx).await?`, `.execute(&tx).await?` — so queries and mutations run inside the active transaction. Committing is explicit: `commit().await?` persists the work and ends the transaction. Dropping the handle without committing rolls the transaction back, so a returned error or a cancelled future leaves no partial work behind.

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

## Transaction Isolation Levels

This section documents the synchronous runtime form of an isolated transaction boundary. Transaction blocks can be configured with explicit SQL isolation levels:

* `ReadCommitted`: Prevents dirty reads (default in PostgreSQL).
* `RepeatableRead`: Prevents non-repeatable reads and phantom reads.
* `Serializable`: Highest isolation level, enforcing strict serializability.

Isolation levels are configured at the transaction boundary:

```rust,no_run
{{#rustdoc_include ../examples/sync_transaction_isolation.rs:isolation}}
```

## Raw Handle Escape Hatch

Synchronous adapters expose the underlying driver handle of the connection, so raw SQL can be executed directly and raw transaction control statements can be issued through it. That access is supported surface rather than a private detail: it is the escape hatch for behaviour the typed API does not model.

A transaction scope, however, owns the bookkeeping. A `BEGIN`, `COMMIT`, `ROLLBACK`, `SAVEPOINT`, `RELEASE SAVEPOINT`, or `ROLLBACK TO SAVEPOINT` issued through the raw handle inside a typed scope forfeits the guarantees of that scope — rollback on error or unwind, savepoint nesting, and commit on success — because the scope no longer observes the state of the transaction it believes it owns.

What such a desynchronisation costs is bounded, and deliberately so:

- A refused `ROLLBACK` — and likewise a refused savepoint rollback or a refused savepoint release — is classified as benign when the driver observes that no transaction is open. The block was already closed through the raw handle, so the connection is exactly as clean as the rollback wanted it to be, and the scope records it clean instead of marking it failed.
- A desynchronised scope therefore costs at most its own transaction call its driver error. It never leaves the adapter unable to open a new typed transaction, and a healthy connection is never wedged behind a failure it cannot clear.

The observation is driver-specific, because the engines disagree on a transaction-control statement issued with no transaction open:

- SQLite refuses `COMMIT` and `ROLLBACK` with an error, so the SQLite adapter classifies the refusal through the driver's autocommit check — the same observation the migration tooling uses to detect a script that left a transaction open. A refusal raised while no transaction is open is benign; a refusal that leaves a transaction open remains the driver's own error and is reported as such.
- PostgreSQL reports both statements in an idle session as warnings rather than errors, so no classification is needed there and the bookkeeping resynchronises through the server's own tolerance.

The classification covers the rollback path only. A `COMMIT` that the raw handle desynchronised still costs its own transaction call its driver error, and the scope is closed by its rollback instead of being reported as successfully committed.
