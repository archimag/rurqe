//! Integration tests for the transactional guarantees of the `r2d2` SQLite pool.
//!
//! Every scenario drives a one-connection in-memory pool, so the connection a
//! block checks out is the connection the next block checks out again: the work
//! a committed block leaves behind, the work a rolled back block discards, and
//! the return of the connection itself are all observed through the pool.

#![cfg(feature = "r2d2")]

use std::panic::{catch_unwind, AssertUnwindSafe};

use r2d2_sqlite::SqliteConnectionManager;
use rurqe_sqlite::rusqlite::{Error, Result};
use rurqe_sqlite::{SqlitePool, SqlitePoolError};

/// The pool every scenario of this file drives.
type Pool = SqlitePool<SqliteConnectionManager>;

/// Builds a one-connection pool over an in-memory database holding `table`.
///
/// A single connection keeps the in-memory database alive across checkouts, so
/// every block observes the work the previous one committed.
fn seeded(table: &str) -> Pool {
    let pool = SqlitePool::new(
        r2d2::Pool::builder()
            .max_size(1)
            .build(SqliteConnectionManager::memory())
            .expect("a one-connection in-memory pool builds"),
    );
    let setup = format!(
        "CREATE TABLE {table} (id INTEGER NOT NULL PRIMARY KEY, balance INTEGER NOT NULL)"
    );
    pool.transaction(|connection| connection.execute_batch(&setup))
        .expect("the fixture block commits");
    pool
}

/// Counts the rows of `table` and reports their summed balance.
fn totals(pool: &Pool, table: &str) -> (i64, i64) {
    let sql = format!("SELECT COUNT(*), COALESCE(SUM(balance), 0) FROM {table}");
    pool.transaction(|connection| {
        connection.query_row(&sql, [], |row| Ok((row.get(0)?, row.get(1)?)))
    })
    .expect("the counting block commits")
}

#[test]
fn a_committed_block_persists_its_work() {
    let pool = seeded("rurqe_pool_commit");

    pool.transaction(|connection| {
        connection.execute_batch("INSERT INTO rurqe_pool_commit (id, balance) VALUES (1, 100)")
    })
    .expect("the block commits");

    // A one-connection pool hands the committed connection back before the
    // counting block can observe the row.
    assert_eq!(totals(&pool, "rurqe_pool_commit"), (1, 100));
}

#[test]
fn a_failing_block_rolls_back_its_work() {
    let pool = seeded("rurqe_pool_rollback");

    let rejected = pool.transaction(|connection| -> Result<()> {
        connection
            .execute_batch("INSERT INTO rurqe_pool_rollback (id, balance) VALUES (1, 100)")?;
        Err(Error::InvalidQuery)
    });
    assert!(
        matches!(&rejected, Err(SqlitePoolError::Query(_))),
        "the block reports the driver failure: {rejected:?}"
    );

    // The row the block inserted is gone and the connection that carried it is
    // back in the pool, which is what lets the counting block run at all.
    assert_eq!(totals(&pool, "rurqe_pool_rollback"), (0, 0));
}

#[test]
fn a_panicking_block_rolls_back_its_work() {
    let pool = seeded("rurqe_pool_panic");

    let unwound = catch_unwind(AssertUnwindSafe(|| {
        pool.transaction(|connection| -> Result<()> {
            connection
                .execute_batch("INSERT INTO rurqe_pool_panic (id, balance) VALUES (1, 100)")?;
            panic!("the block unwinds after inserting its row");
        })
    }));
    assert!(unwound.is_err(), "the block unwinds");

    // The rollback runs while the block unwinds, so the connection returns to
    // the pool clean and the insert is gone.
    assert_eq!(totals(&pool, "rurqe_pool_panic"), (0, 0));
}

#[test]
fn a_nested_block_rolls_back_only_itself() {
    let pool = seeded("rurqe_pool_nested");

    pool.transaction(|connection| -> Result<()> {
        connection
            .execute_batch("INSERT INTO rurqe_pool_nested (id, balance) VALUES (1, 100)")?;

        let nested = connection.transaction(|nested| -> Result<()> {
            nested.execute_batch("INSERT INTO rurqe_pool_nested (id, balance) VALUES (2, 200)")?;
            Err(Error::InvalidQuery)
        });
        assert!(nested.is_err(), "the nested block reports its own failure");

        Ok(())
    })
    .expect("the enclosing block commits");

    // The savepoint undid the nested insert alone, and the enclosing block
    // committed both its own row and the return of the connection.
    assert_eq!(totals(&pool, "rurqe_pool_nested"), (1, 100));
}

#[test]
fn the_connection_returns_to_the_pool_after_every_ending() {
    let pool = seeded("rurqe_pool_return");

    pool.transaction(|connection| {
        connection.execute_batch("INSERT INTO rurqe_pool_return (id, balance) VALUES (1, 100)")
    })
    .expect("the committing block returns its connection");

    let rejected = pool.transaction(|connection| -> Result<()> {
        connection
            .execute_batch("INSERT INTO rurqe_pool_return (id, balance) VALUES (2, 200)")?;
        Err(Error::InvalidQuery)
    });
    assert!(rejected.is_err(), "the failing block reports its own failure");

    let unwound = catch_unwind(AssertUnwindSafe(|| {
        pool.transaction(|connection| -> Result<()> {
            connection
                .execute_batch("INSERT INTO rurqe_pool_return (id, balance) VALUES (3, 300)")?;
            panic!("the block unwinds after inserting its row");
        })
    }));
    assert!(unwound.is_err(), "the panicking block unwinds");

    // Only the first insert survived all three endings, and every ending
    // returned its connection: a checkout that never came back would leave the
    // one-connection pool with nothing to serve the counting block.
    assert_eq!(totals(&pool, "rurqe_pool_return"), (1, 100));
}
