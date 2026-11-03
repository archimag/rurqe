//! Pooled transactions on the synchronous SQLite adapter.
//!
//! Every block below runs inside a pooled transaction, so the example builds
//! the pool and only refers to the functions that own the blocks: the pipeline
//! compiles the file and executes none of it, exactly as a chapter that
//! includes a fragment needs.

use r2d2_sqlite::SqliteConnectionManager;
use rurqe::sqlite::prelude::*;
use rurqe::sqlite::rusqlite;

/// Builds a one-connection pool over an in-memory database holding the table
/// the blocks write to.
///
/// Both halves of that description are load-bearing. `r2d2_sqlite` opens a
/// fresh `rusqlite::Connection` for every connection it manages, and an
/// in-memory connection owns a private, empty database, so the schema created
/// here exists on the single connection this pool holds and nowhere else.
/// `.max_size(1)` is what makes every `transaction` checkout reuse that
/// connection: a larger pool would hand a block a second, schema-less
/// database, and a connection the pool discards — one whose health check
/// fails — is replaced by a fresh, empty one for the same reason. A pool that
/// needs several connections over in-memory SQLite shares one database
/// through a named memory URI instead; this example keeps one connection
/// exactly because it does not.
fn account_pool() -> SqlitePool<SqliteConnectionManager> {
    let pool = SqlitePool::new(
        r2d2::Pool::builder()
            .max_size(1)
            .build(SqliteConnectionManager::memory())
            .unwrap_or_else(|error| panic!("the example pool configuration is valid: {error}")),
    );
    pool.transaction(|connection| {
        connection.execute_batch(
            "CREATE TABLE accounts (id INTEGER NOT NULL PRIMARY KEY, balance INTEGER NOT NULL)",
        )
    })
    .unwrap_or_else(|error| panic!("the example table is created: {error}"));
    pool
}

// ANCHOR: transaction
/// Records both opening balances in one pooled transaction.
///
/// The closure reports the driver error of the runtime, which the pool hands
/// back as its own error. The block commits when the closure returns `Ok` and
/// rolls back when it returns `Err` or unwinds, and the checked-out connection
/// returns to the pool on every one of those paths.
fn record_opening_balances(
    pool: &SqlitePool<SqliteConnectionManager>,
) -> Result<(), SqlitePoolError> {
    pool.transaction(|connection| -> Result<(), rusqlite::Error> {
        connection.execute_batch("INSERT INTO accounts (id, balance) VALUES (1, 100)")?;
        connection.execute_batch("INSERT INTO accounts (id, balance) VALUES (2, 200)")?;
        Ok(())
    })
}
// ANCHOR_END: transaction

// ANCHOR: nesting
/// Records an opening balance around a nested block that moves a second one.
///
/// The inner `transaction` call is a savepoint of the pooled block: returning
/// `Ok` releases it, and returning `Err` or unwinding rolls it back while the
/// enclosing block stays open.
fn record_around_nested_block(
    pool: &SqlitePool<SqliteConnectionManager>,
) -> Result<(), SqlitePoolError> {
    pool.transaction(|connection| -> Result<(), rusqlite::Error> {
        connection.execute_batch("INSERT INTO accounts (id, balance) VALUES (1, 100)")?;

        connection.transaction(|nested| -> Result<(), rusqlite::Error> {
            nested.execute_batch("INSERT INTO accounts (id, balance) VALUES (2, 200)")?;
            Ok(())
        })?;

        Ok(())
    })
}
// ANCHOR_END: nesting

fn main() -> Result<()> {
    // The pool is built over an in-memory database, but no block is entered:
    // the pipeline compiles this example without running it.
    let _pool = account_pool();
    let _ = (record_opening_balances, record_around_nested_block);
    Ok(())
}
