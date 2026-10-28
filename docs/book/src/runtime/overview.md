# Runtime: overview

`rurqe-core` knows no driver. It describes *what* has to be executed, while
the adapter crates `rurqe-postgres` and `rurqe-sqlite` own *how*. The boundary
between the two is fixed by the trait set in
`crates/rurqe-core/src/query/execution.rs`, and no adapter rewrites that
contract on its own.

## The five traits of the execution contract

| Trait | Role | Implemented by |
|---|---|---|
| `ExecutableQuery` | turns a built query into a `BuiltQuery` (SQL + parameters) | `SelectQuery`, `InsertQuery`, `UpdateQuery`, `DeleteQuery`, `GroupedQuery` |
| `QueryBackend` | synchronous execution of a ready statement | `PgConnection`, `PgPool`, `SqliteConnection`, `SqlitePool` |
| `QueryBackendExt` | row decoding (`fetch_all`, `fetch_optional`, `fetch_one`, `execute`) | automatically for every `QueryBackend` |
| `AsyncQueryBackend` | asynchronous execution of a ready statement | `PgAsyncConnection`, `PgAsyncTransaction`, `PgAsyncPool` |
| `AsyncQueryBackendExt` | the same helpers for asynchronous backends | automatically for every `AsyncQueryBackend` |

`ExecutableQuery` is implemented exactly for the builder states that can be
rendered: `SelectQuery<…, HasProjection, Lock>` for every lock state,
`InsertQuery` with a non-empty row source (`Values: InsertSource<T>`),
`UpdateQuery` only when `Assignments: AssignmentsPresent` and
`State: ExecutableMutation`, `DeleteQuery` when `State: ExecutableMutation`,
and `GroupedQuery`. Everything outside those boundaries does not compile.

## The synchronous backend

```rust
# extern crate rurqe;
# use rurqe::prelude::*;
pub trait QueryBackend {
    type Error;

    fn fetch_raw(&mut self, query: &BuiltQuery) -> Result<Vec<Vec<SqlVal>>, Self::Error>;

    fn fetch_raw_limited(
        &mut self,
        query: &BuiltQuery,
        limit: Option<usize>,
    ) -> Result<Vec<Vec<SqlVal>>, Self::Error>;

    fn execute_raw(&mut self, query: &BuiltQuery) -> Result<u64, Self::Error>;
}
# fn main() {}
```

`fetch_raw_limited` has a default implementation: it calls `fetch_raw` and
truncates the result. All three built-in synchronous adapters override the
method and hand the limit to the driver itself — `postgres` through
`query_raw(...).take(limit)`, `rusqlite` by stopping the `rows.next()` loop — so
a query that needs one row never materializes the whole set.

## The asynchronous backend

```rust
# extern crate rurqe;
# use core::future::Future;
# use rurqe::prelude::*;
pub trait AsyncQueryBackend: Sync {
    type Error: Send;

    fn fetch_raw(
        &self,
        query: &BuiltQuery,
    ) -> impl Future<Output = Result<Vec<Vec<SqlVal>>, Self::Error>> + Send;

    fn fetch_raw_limited(
        &self,
        query: &BuiltQuery,
        limit: Option<usize>,
    ) -> impl Future<Output = Result<Vec<Vec<SqlVal>>, Self::Error>> + Send;

    fn execute_raw(
        &self,
        query: &BuiltQuery,
    ) -> impl Future<Output = Result<u64, Self::Error>> + Send;
}
# fn main() {}
```

The backend is taken by `&self` and requires `Sync`: a connection is shared
between tasks, and the returned futures are explicitly `+ Send`, so a query can
move into `tokio::spawn`. The limit travels into `tokio_postgres::RowStream`,
which is polled row by row and stops at the requested count.

## Row decoding

The `QueryBackendExt` / `AsyncQueryBackendExt` helpers apply one policy:

- `fetch_all` — reads every row and decodes each through
  `FromRow::try_from_row`; a decode failure carries the row index in
  `ExecutionError::RowDecode { row_index, source }`.
- `fetch_optional` — requests **at most two** rows
  (`fetch_raw_limited(&built, Some(2))`). Zero rows — `Ok(None)`, one — decoding,
  more — `ExecutionError::MultipleRows { count }` *before* any attempt to decode
  a row.
- `fetch_one` — `fetch_optional(..)?.ok_or(ExecutionError::NoRows)`.
- `execute` — renders and calls `execute_raw`, returning the affected row count.

A query that by contract returns one row therefore cannot physically pull the
whole set from the network, and the "too many rows" error arises without
decoding the extra row.

## The execution error

```rust
# extern crate rurqe;
# use rurqe::prelude::*;
pub enum ExecutionError<E> {
    Build(BuildError),
    Backend(E),
    RowDecode { row_index: usize, source: RowDecodeError },
    NoRows,
    MultipleRows { count: usize },
}
# fn main() {}
```

The practical consequence of the two-tier architecture: `ExecutionError::Build`
arises **before** any network I/O — it is the rejection of the AST pre-flight
validation (`BuildError`), not a server answer. The `From<E> for ExecutionError<E>`
implementation wraps a driver error in `Backend`, so `?` works in application
code without manual mapping.

Inspection helpers: `is_build` / `as_build`, `is_not_found`, `is_multiple_rows`,
`is_row_decode` / `as_row_decode`, `as_backend` / `into_backend`.

## Adapters

| Adapter | Crate | Cargo feature | Contract |
|---|---|---|---|
| `PgConnection` | `rurqe-postgres` | `postgres` (or `r2d2`) | `QueryBackend`, `TransactionBackend`, `TransactionalConnection` |
| `PgAsyncConnection` | `rurqe-postgres` | `tokio-postgres` | `AsyncQueryBackend` |
| `PgAsyncTransaction` | `rurqe-postgres` | `tokio-postgres` | `AsyncQueryBackend` |
| `PgAsyncPool` | `rurqe-postgres` | `deadpool-postgres` | `AsyncQueryBackend` |
| `PgPool<M>` | `rurqe-postgres` | `r2d2` | `QueryBackend` |
| `SqliteConnection` | `rurqe-sqlite` | `sqlite` | `QueryBackend`, `TransactionBackend`, `TransactionalConnection` |
| `SqlitePool<M>` | `rurqe-sqlite` | `r2d2` | `QueryBackend` |

The dialect part (`Postgres`, `Pg14/15/16`, `Sqlite`, `Sqlite3_0 … Sqlite3_38`)
compiles under any adapter feature, so a typed query can be composed and
rendered without pulling a driver.

## Next

- [Transactions and savepoints](transactions.md)
- [Connection pools](pools.md)
- [Ecosystem types](types.md)
- [Custom types](custom_types.md)
