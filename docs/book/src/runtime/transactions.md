# Transactions and savepoints

The synchronous adapters (`PgConnection`, `SqliteConnection`) do not implement
transactions each in its own way. The `BEGIN` / `SAVEPOINT` / `ROLLBACK` policy
lives in one place — `TransactionGuard` in
`crates/rurqe-core/src/query/transaction.rs` — and the adapter only provides
state accounting through `TransactionalConnection`.

## State accounting: `TransactionStatus`

```rust
# extern crate rurqe;
# use rurqe::core::query::TransactionContext;
pub struct TransactionStatus {
    context: TransactionContext,
    open: bool,
    poisoned: bool,
}
# fn main() {}
```

- `is_open()` — whether an outer transaction is open;
- `is_poisoned()` — whether a `ROLLBACK` the driver refused to execute is still
  pending;
- `depth()` — the number of open savepoints;
- `current_savepoint()` — the name of the innermost savepoint.

`TransactionContext` numbers nested blocks: names are generated as `rurqe_sp_1`,
`rurqe_sp_2`, … by `SavepointName::generated`, and `SavepointName` accepts only
`[A-Za-z0-9_]`, so a name can never inject SQL into the rendered statement.

## Opening a block

```rust,ignore
pub fn begin(connection: &'a mut B) -> Result<Self, B::Error>;
pub fn begin_with(
    connection: &'a mut B,
    isolation: Option<IsolationLevel>,
) -> Result<Self, B::Error>;
```

The logic of `begin_with`:

1. `recover(connection)` — when the connection is poisoned, the abandoned
   `ROLLBACK` is retried. Success clears the state; a second driver refusal is
   returned to the caller, because such a connection has to be discarded rather
   than reused.
2. If a transaction is already open, a `SAVEPOINT` is opened and the guard is
   marked nested (`outer: false`). A `SAVEPOINT` failure removes the name from
   the accounting and returns the error.
3. Otherwise `TransactionBackend::begin(isolation)` is called, the state is
   marked open, and the guard becomes the outer one (`outer: true`).

## Rendered statements

| Action | Outer block | Nested block |
|---|---|---|
| opening | `BEGIN` | `SAVEPOINT rurqe_sp_N` |
| opening with a level | `BEGIN ISOLATION LEVEL <LEVEL>` | `SAVEPOINT rurqe_sp_N` (the level is ignored) |
| `commit()` | `COMMIT` | `RELEASE SAVEPOINT rurqe_sp_N` |
| `Drop` without `commit()` | `ROLLBACK` | `ROLLBACK TO SAVEPOINT rurqe_sp_N`, then `RELEASE SAVEPOINT rurqe_sp_N` |

`IsolationLevel::as_str` returns exactly `READ COMMITTED`, `REPEATABLE READ`, or
`SERIALIZABLE`. By default `TransactionBackend::begin` assembles the string
`BEGIN` and appends ` ISOLATION LEVEL …` only when a level is given.

## The isolation level belongs to the outer transaction only

```rust
# extern crate rurqe;
pub enum IsolationLevel {
    ReadCommitted,
    RepeatableRead,
    Serializable,
}
# fn main() {}
```

SQL does not define per-savepoint isolation, so `begin_with` called inside an
already open transaction creates a plain `SAVEPOINT` and ignores the requested
level: the outer transaction keeps its own. This is pinned by the test
`a_nested_block_ignores_the_requested_isolation_level`, which asserts that the
statement list holds no `ISOLATION LEVEL` occurrence at all.

## Rollback, commit, and connection poisoning

`commit(self)` calls `release()` and then marks the guard finished. For the
outer block `release` issues `COMMIT` and closes the state; for a nested block it
issues `RELEASE SAVEPOINT` and decrements the depth. `Drop` of a finished guard
does nothing.

When a guard is destroyed without `commit()`:

- an outer block issues `ROLLBACK`; on success the state is closed and the
  nesting reset, while a driver refusal **poisons** the connection
  (`mark_poisoned`): the work of the block may not be cancelled yet, so the
  transaction cannot be recorded as closed;
- a nested block issues `ROLLBACK TO SAVEPOINT` + `RELEASE SAVEPOINT`; if the
  rollback fails, the depth is kept and the connection is poisoned as well.

A poisoned connection is not "a closed transaction" but an explicitly unknown
state: the next `begin` first tries to finish the rollback through `recover`.
That rules out starting a new transaction on top of an unfinished one.

## Observability

The guard's events are emitted into the `tracing` facade behind
`#[cfg(feature = "tracing")]`: `debug!` on savepoint creation and release,
`info!` on `BEGIN`/`COMMIT`, `warn!` on rollback and on a successful recovery
after a failed rollback, `error!` on a repeated refusal. With the feature off,
`rurqe-core` does not pull `tracing` into the dependency graph.

## Ergonomic wrappers

`PgConnection`:

```rust,ignore
pub fn transaction<T, E, F>(&mut self, f: F) -> Result<T, E>
where
    F: FnOnce(&mut PgConnection) -> Result<T, E>,
    E: From<PgError>;

pub fn transaction_with_isolation<T, E, F>(
    &mut self,
    isolation: IsolationLevel,
    f: F,
) -> Result<T, E>
where
    F: FnOnce(&mut PgConnection) -> Result<T, E>,
    E: From<PgError>;
```

`SqliteConnection::transaction` has the same shape with `E: From<rusqlite::Error>`.
The SQLite adapter overrides `TransactionBackend::begin` and executes
`execute_batch("BEGIN")`, ignoring the isolation level: SQLite serializes access
to the database with a file lock, and the isolation level does not map onto a
separate statement. This is a deliberate decision, not a loss of functionality —
see [Migration concurrency](../dialects/sqlite/concurrency.md).

The asynchronous driver relies on `tokio_postgres::Transaction`: `commit()` and
`rollback()` are called explicitly, and `Drop` without a commit rolls the
transaction back through the driver, so a cancelled future or a panic leaves no
unfinished work.

## Guarantees of the design

| Invariant | Where it is pinned |
|---|---|
| A panic inside a block rolls the transaction back | `Drop for TransactionGuard` |
| A driver refusal of `ROLLBACK` is not masked as success | `mark_poisoned` + `recover` |
| A savepoint name cannot contain SQL | `SavepointName::new` (only `[A-Za-z0-9_]`) |
| A nested block does not change the isolation level | `begin_with` + a test in `transaction.rs` |
| The isolation level reaches only the outer `BEGIN` | `TransactionBackend::begin` |
