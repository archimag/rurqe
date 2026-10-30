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

```rust,no_run
{{#rustdoc_include ../examples/sync_transactions.rs:transaction}}
```

## Transaction Isolation Levels
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

Synchronous adapters provide direct access to the underlying database driver handles (`rusqlite::Connection` via `Deref`/`DerefMut` on `SqliteConnection`, and `client`/`client_mut` on `PgConnection`).

Issuing raw transaction control statements (`BEGIN`, `COMMIT`, `ROLLBACK`) directly through raw driver handles bypasses `TransactionGuard` bookkeeping. To protect connection health, adapters treat benign rollback refusals as non-fatal when a transaction was closed externally, preventing connection poisoning.
