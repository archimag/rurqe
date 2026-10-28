# Connection pools

A pool is a backend too. Application code does not distinguish a pool from a
connection: as soon as a type implements `QueryBackend` or `AsyncQueryBackend`,
the `fetch_all`, `fetch_optional`, `fetch_one`, and `execute` helpers of the
`*Ext` traits appear on it for free.

## The asynchronous pool: `PgAsyncPool` (deadpool-postgres)

Feature: `deadpool-postgres`. It is `Clone` and implements `AsyncQueryBackend`.

```rust,ignore
pub struct PgAsyncPool {
    pool: deadpool_postgres::Pool,
}

impl PgAsyncPool {
    pub fn new(pool: deadpool_postgres::Pool) -> Self;
    pub fn pool(&self) -> &deadpool_postgres::Pool;
}
```

Every call checks a connection out of the pool, awaits execution, and returns
the connection. `fetch_raw_limited` passes the limit into the `tokio_postgres`
row stream, so a single-row lookup stops at the second row.

```rust
use rurqe::postgres::prelude::*;

let rows = pool
    .fetch_all(&select_from(Users).select(cols!(UsersId)))?;
```

A pool error lands in `PgError::Pool`:

```rust,ignore
impl From<deadpool_postgres::PoolError> for PgError {
    fn from(error: deadpool_postgres::PoolError) -> Self {
        PgError::Pool(error)
    }
}
```

so `?` in asynchronous code propagates both a pool error and a driver error into
one type.

## The synchronous PostgreSQL pool: `PgPool<M>` (r2d2)

Feature: `r2d2`. The pool is parameterized by an `r2d2` connection manager.

```rust,ignore
pub struct PgPool<M: ManageConnection> {
    pool: r2d2::Pool<M>,
}

impl<M: ManageConnection<Connection = postgres::Client>> PgPool<M> {
    pub fn new(pool: r2d2::Pool<M>) -> Self;
    pub fn get(&self) -> Result<r2d2::PooledConnection<M>, r2d2::Error>;
}
```

The errors are kept apart explicitly:

```rust,ignore
pub enum PgPoolError {
    Pool(r2d2::Error),
    Query(PgError),
}
```

`fetch_raw_limited` passes the limit into the `postgres` row iterator
(`query_raw(...).take(limit)`), so the semantics match `PgConnection`.

## The synchronous SQLite pool: `SqlitePool<M>` (r2d2)

Feature: `r2d2`. The manager is any `r2d2` manager over `rusqlite::Connection`.

```rust,ignore
pub struct SqlitePool<M: ManageConnection> {
    pool: r2d2::Pool<M>,
}

impl<M: ManageConnection<Connection = ::rusqlite::Connection>> SqlitePool<M> {
    pub fn new(pool: r2d2::Pool<M>) -> Self;
    pub fn get(&self) -> Result<r2d2::PooledConnection<M>, r2d2::Error>;
}
```

```rust,ignore
pub enum SqlitePoolError {
    Pool(r2d2::Error),
    Query(::rusqlite::Error),
}
```

The crate documentation example builds and runs as a doctest with the `r2d2`
feature enabled:

```rust,ignore
use r2d2_sqlite::SqliteConnectionManager;

let manager = SqliteConnectionManager::memory();
let pool = r2d2::Pool::builder().max_size(4).build(manager)?;
let _pool = rurqe_sqlite::SqlitePool::new(pool);
```

## Choosing an adapter

| Scenario | Adapter |
|---|---|
| an asynchronous service on `tokio` | `PgAsyncPool` (`deadpool-postgres`) |
| a synchronous worker / CLI on `postgres` | `PgPool<M>` (`r2d2`) |
| a synchronous service on SQLite | `SqlitePool<M>` (`r2d2`) |
| one long-lived session | `PgConnection` / `PgAsyncConnection` / `SqliteConnection` |

Transactions are arranged on a concrete connection (`PgConnection::transaction`),
not on a pool: a pool hands out a connection for the duration of a call, while
`TransactionGuard` holds `&mut` for the whole block.

## Summary

- A pool implements the same trait as a connection, so switching does not change
  application code.
- The row limit reaches the driver in every built-in adapter.
- Pool errors and query errors are separate variants of one enum.
