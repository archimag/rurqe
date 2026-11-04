# Connection Pools

A pool keeps a set of open sessions and lends one of them to the work that needs it. RURQE wraps three driver pools, and the wrapper implements the execution contract of the library, so the terminal calls of a query run directly against the pool handle: the call checks a connection out, runs the statement, and returns the connection once the statement settled.

| Handle | Cargo feature | Driver it pools |
|---|---|---|
| `PgAsyncPool` | `deadpool-postgres` | the asynchronous `tokio-postgres` driver |
| `PgPool` | `r2d2` | the synchronous `postgres` driver |
| `SqlitePool` | `r2d2` | `rusqlite` |

The synchronous wrappers clone without rebuilding the pool, because every clone addresses the one underlying pool, and `pool()` hands back the wrapped driver pool for configuration or inspection performed outside the adapter.

## Adapting an existing pool

An application assembles its pool before it hands it over: the size, the TLS connector, the timeouts, and the recycling policy are settings of the pool itself, and none of them is a property of a typed query. `PgAsyncPool::new(pool)` is the one adaptation point — it stores the finished `deadpool_postgres::Pool` as it is, without rebuilding or reconfiguring it — while `pool()` hands the same driver pool back for a setting or an observation the adapter does not model:

```rust,no_run
{{#rustdoc_include ../examples/pool_transactions.rs:adapt_pool}}
```

The statement renders:

```sql
SELECT "id" AS "account_id", "balance" AS "account_balance" FROM "accounts" WHERE ("balance" > $1)
```

A terminal call on the adapter checks a connection out of that pool, runs against the session it received, and returns the checkout once the statement settled. Wrapping a pool changes where the sessions come from, not how a call on them behaves.

### Where the guarantee of the adapter ends

The guarantees this chapter documents are guarantees about the checkouts the adapter mediates. A checkout the caller takes from the wrapped pool and drives itself — `pool().get().await?.transaction().await?` — is the driver's own transaction: the server opens and closes it, but the adapter never saw that checkout, so its session is not proven idle before the work runs, the savepoint nesting of the closure form does not apply inside it, and a refused control statement is recorded nowhere. An application therefore routes its transactional surface through `PgAsyncPool::transaction`, or through `transaction_with_isolation` when a block needs an isolation level, and keeps the wrapped pool for the settings and the observations the adapter does not model.

## A statement on a pool

A single statement is executed by checking a connection out, proving its session idle, running the statement in autocommit, and returning the connection once the statement settled. A statement the driver refused is settled as well, so the checkout returns to the pool on that path too.

What the proof and the return cost follows from what a driver can report about a session, which is why each adapter states it differently: the asynchronous adapter proves the session before it uses it, while the synchronous adapters arrange the state or run the reset themselves.

## The idle proof of the asynchronous pool

The asynchronous adapter proves the session before the work runs: one `ROLLBACK` through the simple query protocol precedes the `BEGIN` of a block and precedes the dispatch of a statement. PostgreSQL answers that statement in every session state — a session that still holds a transaction has it ended, including the aborted one where `BEGIN` is rejected with `25P02`, and an idle session answers with the warning `25P01` instead of an error — so the proof never has to classify the state the session arrived in. A proof the driver refuses destroys the checkout and surfaces as `PgError::AbandonedTransaction`, so a session the pool cannot vouch for never reaches another caller.

A statement whose future is dropped before it settles is never settled by its own `Drop`, which cannot await a proof: the connection is taken out of the pool and destroyed, and the pool opens a fresh one for the next caller. A transaction block is bounded the same way: a block that is cancelled or unwound never reaches the end of the call, so neither `COMMIT` nor `ROLLBACK` is awaited for it, the checkout is destroyed instead of returned, dropping it closes the session, the server rolls the abandoned transaction back when that session ends, and the pool replaces the connection.

A transaction-control statement issued through a single-statement path leaves a transaction open on the session. The idle proof of the next mediated checkout — a block or a statement — ends it before that checkout runs, so the abandoned transaction is neither inherited nor committed by a later caller.

## The repair of the synchronous PostgreSQL pool

A synchronous client exposes no transaction state, so the synchronous PostgreSQL pool cannot ask a session whether it is idle. It arranges for every connection to be returned to the pool in an idle state instead, which is why a checkout costs no round trip of its own: a statement path requires its statement to run in autocommit, and a block the pool drives ends with an awaited `COMMIT` or `ROLLBACK`, so a checkout the pool drove to a clean ending returns without a repair.

A session the caller reached through the borrowed handle of the checkout is the other case: the statements issued there stay outside the bookkeeping of the pool, so the proof is forfeited as soon as the handle is borrowed mutably, and the reset runs when the checkout ends. The reset is one round trip of the simple protocol carrying a single `ROLLBACK`, the statement PostgreSQL answers in every session state, so a session that arrived with a transaction still open has it ended before it returns to the pool.

A repair the driver refuses leaves the session in a state the pool cannot vouch for, so the connection is never handed to the next caller: the pool reports the failure in its log, discards the connection once the driver marks it closed, and opens a fresh connection for the next caller. The failure itself is the `PgError::AbandonedTransaction` value, which keeps the driver failure whole and hands it back through `std::error::Error::source()`.

## The reset of the SQLite pool

The synchronous SQLite pool returns every connection to an idle session before handing it out, and it reads the engine's own observation of the session instead of arranging the state: a connection that is already outside a transaction issues no statement at all, while one that still holds a transaction — opened through the raw handle, or left open by a block whose rollback the driver refused — has it ended by a single `ROLLBACK`. A connection whose `ROLLBACK` the driver refuses is never handed out: the checkout fails with `SqlitePoolError::AbandonedTransaction` carrying the driver failure, instead of running the work of the next caller inside the abandoned transaction.

`SqlitePool::get()` performs that reset and hands the checked-out connection back, and every pooled statement and every pooled block enters through it.

## Atomic blocks on a pool

Several statements become one atomic block through the closure form, which keeps one connection checked out for the whole duration of the block:

```rust,no_run
{{#rustdoc_include ../examples/pool_transactions.rs:transaction}}
```

- The outermost block of a pooled transaction runs between `BEGIN` and `COMMIT`, and a block opened while a transaction is already open nests inside a `SAVEPOINT`.
- A block that returns `Err` has its statements rolled back before that error reaches the caller, and a block that unwinds is rolled back the same way.
- The checkout returns to the pool however the block ends, whether it committed or was rolled back, so several blocks of a shared pool run concurrently, each on its own connection.
- A `ROLLBACK` the driver refuses supersedes the error of the block and surfaces as `PgError::ConnectionPoisoned`, because the transaction state is unknown from that point on.
- A block whose connection is poisoned before its commit is not committed: the adapter rolls its statements back and reports `PgError::ConnectionPoisoned`, and the poisoned checkout is destroyed rather than returned to the pool.

A block opened on the scope of an open transaction is a savepoint of that transaction, so a nested block loses its own work and nothing else:

```rust,no_run
{{#rustdoc_include ../examples/pool_transactions.rs:nesting}}
```

The asynchronous pool also opens an isolated boundary, where the level reaches the statement that opens the outermost block — `BEGIN ISOLATION LEVEL <level>` — so every statement of the block observes it:

```rust,no_run
{{#rustdoc_include ../examples/pool_transactions.rs:isolation}}
```

A block opened while a transaction is already open nests inside a savepoint, and SQL defines no per-savepoint isolation, so the enclosing transaction keeps the level it was opened with and the requested one applies to the outermost block alone.

The synchronous pool hands its closure the checked-out connection, and the closure reports the driver error of that runtime:

```rust,no_run
{{#rustdoc_include ../examples/sync_pool_transactions.rs:transaction}}
```

The block commits when the closure returns `Ok` and rolls back when it returns `Err` or unwinds, exactly as the owned connection does, and the checkout returns to the pool on every one of those paths. A block opened inside a pooled block is a savepoint of it again:

```rust,no_run
{{#rustdoc_include ../examples/sync_pool_transactions.rs:nesting}}
```

Returning `Ok` releases the savepoint of the nested block; returning `Err` or unwinding rolls it back, and the enclosing block stays open to commit its own work. The synchronous PostgreSQL pool opens an isolated boundary through `transaction_with_isolation`, and the SQLite pool offers none: SQLite serialises access through database-level locking, so its `BEGIN` ignores an isolation level by design.

## Errors of a pooled call

| Handle | Error type | Failures it reports |
|---|---|---|
| `PgAsyncPool` | `PgError` | `Pool` for a checkout the pool could not hand out, `Driver` for a statement the server refused, `ConnectionPoisoned` when the transaction state cannot be proven, and `AbandonedTransaction` when the session could not be proven idle |
| `PgPool` | `PgPoolError` | `Pool(r2d2::Error)` for a checkout, and `Query(PgError)` for the block or the statement |
| `SqlitePool` | `SqlitePoolError` | `Pool(r2d2::Error)` for a checkout, `Query(rusqlite::Error)` for the block or the statement, and `AbandonedTransaction(rusqlite::Error)` for a session that could not be reset |

Either pool type converts from the failures its own paths raise — `From<r2d2::Error>`, `From<rusqlite::Error>`, and `From<PgError>` — so a checkout failure and a block failure propagate with `?` into a function returning the pool error type. The three types are open enumerations, so a match over one of them carries a wildcard arm, and `PgError`'s own variant set follows the adapters a build compiles.
