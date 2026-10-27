# Drivers

`rurqe-postgres` ships two adapters over the same built-query contract:
asynchronous over `tokio-postgres` and synchronous over `postgres`. Both
convert rows through one value layer (`backend/types.rs`), so a `SqlVal`
round-trips identically whichever runtime executes the statement.

## Asynchronous: `tokio-postgres`

* `PgAsyncConnection::new(client)` wraps a `tokio_postgres::Client`;
  `client()` shares it.
* `transaction().await` returns a `PgAsyncTransaction`: `commit()` is
  explicit, and dropping the handle rolls back — the driver issues the
  `ROLLBACK` from `Drop` without awaiting, so an error, a panic, and a
  cancelled future all leave the connection without an open transaction.
* The adapter implements `AsyncQueryBackend` for the connection **and** for
  the open transaction, so `fetch_all` / `fetch_optional` / `fetch_one` /
  `execute` behave identically inside and outside a transaction. The helper
  accepts any `tokio_postgres::GenericClient`, so a pooled client works
  without unwrapping.
* A limited fetch streams rows through `query_raw` and stops once the limit
  is reached — the remaining rows of an unbounded result set are never read
  from the socket.

## Synchronous: `postgres`

* `PgConnection::new(client)` wraps a `postgres::Client`; `client()`,
  `client_mut()`, and `into_inner()` expose it.
* `transaction(|tx| …)` runs a closure between `BEGIN` and `COMMIT`: an
  `Err` return rolls back, a nested call opens a `SAVEPOINT`, and a
  panicking block is rolled back too, so unwinding never leaves an open
  transaction or savepoint behind.
* `transaction_with_isolation(level, |tx| …)` opens the outermost block
  with `BEGIN ISOLATION LEVEL <level>`; nested blocks ignore the level,
  because SQL defines no per-savepoint isolation. The levels:
  `ReadCommitted`, `RepeatableRead`, `Serializable`.
* A driver that refuses the rollback poisons the connection; the next
  transaction retries the abandoned rollback rather than running on top
  of an unknown one.
* A limited fetch uses the driver's row iterator and `take(limit)`.

## Pooling

Pooled clients implement the same execution contracts — `deadpool-postgres`
for the asynchronous driver, `r2d2` for the synchronous one — so a pooled
connection is passed to `fetch_all` / `execute` directly, without adapter
code in the application.

## Tracing

The asynchronous adapter emits transaction lifecycle events through the
`tracing` facade — beginning, committing, and rolling back a transaction —
while the query core itself stays observability-free.
