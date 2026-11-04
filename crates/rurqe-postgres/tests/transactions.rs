//! Integration tests for the transactional guarantees of the PostgreSQL backend
//! surfaces: the synchronous `r2d2` pool, the asynchronous pool closures, and
//! the asynchronous connection scope.
//!
//! Every scenario needs a live server named by the `RURQE_TEST_POSTGRES_URL`
//! environment variable and reports a skip when it is absent, so the suite runs
//! to completion on a machine without PostgreSQL while a configured environment
//! exercises the whole set. Each scenario owns its fixture table, because the
//! scenarios of one binary run concurrently and a shared table would let them
//! observe each other's work.
//!
//! The regression scenarios pin the disposition of a checkout whose transaction
//! was abandoned. On the synchronous pool a leaked transaction and a leaked
//! aborted transaction are repaired by the reset of the next checkout, while a
//! session that lost its backend is caught at the checkout: the probe of
//! the pool beneath replaces the dead connection, and with that probe
//! turned off the loss reaches the statement of the caller; on the
//! asynchronous surfaces
//! a cancelled block, a panicking block, and a refused rollback destroy their
//! own checkout instead of returning it to the pool, and a transaction handle
//! dropped without a terminal call poisons the connection it borrows, and so does
//! an entry whose `BEGIN` the driver refused or whose future was dropped before
//! the transaction opened.

// A skipped scenario reports itself to the operator running the tests, which is
// the one place the workspace-wide printing ban is lifted for this file.
#![allow(clippy::print_stdout)]
#![cfg(any(feature = "r2d2", feature = "tokio-postgres"))]

/// Helpers shared by the scenarios of this file.
mod support {
    #[cfg(feature = "tokio-postgres")]
    use rurqe_postgres::rurqe_core::expr::SqlVal;
    #[cfg(feature = "tokio-postgres")]
    use rurqe_postgres::rurqe_core::query::{AsyncQueryBackend, BuiltQuery};
    #[cfg(feature = "tokio-postgres")]
    use rurqe_postgres::rurqe_core::row::FromSqlValue;
    #[cfg(feature = "tokio-postgres")]
    use rurqe_postgres::PgError;

    /// The URL of the server the scenarios run against.
    ///
    /// A scenario that cannot run reports itself and returns, so an environment
    /// without PostgreSQL runs the suite to completion instead of failing it.
    pub(super) fn database_url(scenario: &str) -> Option<String> {
        match std::env::var("RURQE_TEST_POSTGRES_URL") {
            Ok(url) if !url.trim().is_empty() => Some(url),
            _ => {
                println!("skipping `{scenario}`: RURQE_TEST_POSTGRES_URL is not set");
                None
            }
        }
    }

    /// Renders a parameterless statement the typed builders do not cover.
    #[cfg(feature = "tokio-postgres")]
    pub(super) fn statement(sql: &str) -> BuiltQuery {
        BuiltQuery {
            sql: sql.to_owned(),
            params: Vec::new(),
        }
    }

    /// Runs a parameterless statement through an asynchronous backend.
    #[cfg(feature = "tokio-postgres")]
    pub(super) async fn execute<B>(backend: &B, sql: &str) -> Result<(), PgError>
    where
        B: AsyncQueryBackend<Error = PgError>,
    {
        AsyncQueryBackend::execute_raw(backend, &statement(sql)).await?;
        Ok(())
    }

    /// Reads the row count and the summed amount of `table`.
    #[cfg(feature = "tokio-postgres")]
    pub(super) async fn totals<B>(backend: &B, table: &str) -> (i64, i64)
    where
        B: AsyncQueryBackend<Error = PgError>,
    {
        let sql = format!("SELECT COUNT(*), COALESCE(SUM(amount), 0) FROM {table}");
        let rows = AsyncQueryBackend::fetch_raw(backend, &statement(&sql))
            .await
            .expect("the counting statement succeeds");

        let mut values = rows
            .into_iter()
            .next()
            .expect("the counting statement returns one row")
            .into_iter();
        let count = values
            .next()
            .and_then(|value| i64::from_sql_value(&value).ok())
            .expect("the count column holds an integer");
        let amount = values
            .next()
            .and_then(|value| i64::from_sql_value(&value).ok())
            .expect("the sum column holds an integer");
        (count, amount)
    }

    /// Reads the single text value a statement such as
    /// `SELECT current_setting('transaction_isolation')` produces.
    #[cfg(feature = "tokio-postgres")]
    pub(super) fn single_text(rows: Vec<Vec<SqlVal>>, sql: &str) -> Result<String, PgError> {
        let value = rows
            .into_iter()
            .next()
            .and_then(|row| row.into_iter().next())
            .ok_or_else(|| PgError::InvalidValue {
                column_type: String::from("text"),
                reason: format!("`{sql}` returned no row with a single column"),
            })?;
        String::from_sql_value(&value).map_err(|failure| PgError::InvalidValue {
            column_type: String::from("text"),
            reason: failure.to_string(),
        })
    }
    /// The failure a block returns when the scenario only needs an error value.
    ///
    /// The variant names a decoding failure because it is the one `PgError`
    /// constructor the adapter exposes to its callers: the scenario never lets
    /// the value reach a column, it only makes the block return `Err` on its
    /// own, so the rollback the adapter awaits is the call that fails next.
    #[cfg(feature = "tokio-postgres")]
    pub(super) fn deliberate_failure() -> PgError {
        PgError::InvalidValue {
            column_type: String::from("text"),
            reason: String::from("the block failed before its rollback"),
        }
    }
}

#[cfg(feature = "r2d2")]
mod sync_pool {
    use std::panic::{catch_unwind, AssertUnwindSafe};
    use std::time::{Duration, Instant};

    use postgres::NoTls;
    use r2d2_postgres::PostgresConnectionManager;
    use rurqe_postgres::rurqe_core::query::{BuiltQuery, QueryBackend};
    use rurqe_postgres::{PgError, PgPool, PgPoolError};

    use crate::support;

    /// The synchronous pool the scenarios of this module drive.
    type Pool = PgPool<PostgresConnectionManager<NoTls>>;

    /// Builds a one-connection pool over the server the scenario names.
    fn pool(url: &str) -> Pool {
        let config: postgres::Config = url.parse().expect("the test URL parses");
        let manager = PostgresConnectionManager::new(config, NoTls);
        PgPool::new(
            r2d2::Pool::builder()
                .max_size(1)
                .build(manager)
                .expect("a one-connection pool builds"),
        )
    }

    /// Resets `table` to an empty fixture through the pool.
    fn reset(pool: &Pool, table: &str) {
        let setup = format!(
            "DROP TABLE IF EXISTS {table}; \
             CREATE TABLE {table} (id INTEGER PRIMARY KEY, amount INTEGER NOT NULL)"
        );
        pool.transaction(|tx| {
            tx.client_mut().batch_execute(&setup)?;
            Ok(())
        })
        .expect("the fixture block commits");
    }

    /// Counts the rows of `table` and reports their summed amount.
    fn totals(pool: &Pool, table: &str) -> (i64, i64) {
        let sql = format!("SELECT COUNT(*), COALESCE(SUM(amount), 0) FROM {table}");
        pool.transaction(|tx| {
            let row = tx.client_mut().query_one(&sql, &[])?;
            Ok((row.get::<_, i64>(0), row.get::<_, i64>(1)))
        })
        .expect("the counting block commits")
    }
    /// Terminates `backend` and waits until the server reports it gone.
    ///
    /// `pg_terminate_backend` returns when the termination is queued, not when
    /// the backend has processed it, so a caller that proceeds immediately
    /// races the death of the session it wants to observe. The poll reads
    /// `pg_stat_activity` until the process leaves it: a role sees the
    /// backends of its own sessions, so the probe needs no privileges beyond
    /// the ones the test URL already carries, and once the row is gone the
    /// terminated backend no longer serves its connection.
    fn terminate_and_await(killer: &mut postgres::Client, backend: i32) {
        let terminated = killer
            .query_one("SELECT pg_terminate_backend($1)", &[&backend])
            .expect("the termination request is dispatched");
        assert!(
            terminated.get::<_, bool>(0),
            "the server accepted the termination of backend {backend}"
        );

        // The poll bounds itself, so a backend that never dies fails the
        // scenario with a diagnostic instead of hanging the suite.
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let listed = killer
                .query_opt("SELECT 1 FROM pg_stat_activity WHERE pid = $1", &[&backend])
                .expect("the activity probe runs");
            if listed.is_none() {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "backend {backend} stayed in pg_stat_activity past the poll budget"
            );
            std::thread::sleep(Duration::from_millis(25));
        }
    }

    /// Builds a one-connection pool that hands its connections out unprobed.
    ///
    /// `r2d2` checks every connection it hands out by default
    /// (`test_on_check_out`), which silently replaces a connection whose
    /// session died. The scenario that observes a dead session reaching the
    /// caller therefore disables the probe, so the checkout hands the
    /// connection out as it is and the caller's own statement meets the
    /// failure.
    fn unprobed_pool(url: &str) -> Pool {
        let config: postgres::Config = url.parse().expect("the test URL parses");
        let manager = PostgresConnectionManager::new(config, NoTls);
        PgPool::new(
            r2d2::Pool::builder()
                .max_size(1)
                .test_on_check_out(false)
                .build(manager)
                .expect("a one-connection pool builds"),
        )
    }
    #[test]
    fn a_committed_block_persists_its_work() {
        let Some(url) = support::database_url("PgPool::transaction commit") else {
            return;
        };
        let pool = pool(&url);
        reset(&pool, "rurqe_tx_commit");

        pool.transaction(|tx| {
            tx.client_mut()
                .batch_execute("INSERT INTO rurqe_tx_commit (id, amount) VALUES (1, 100)")?;
            Ok(())
        })
        .expect("the block commits");

        assert_eq!(totals(&pool, "rurqe_tx_commit"), (1, 100));
    }

    #[test]
    fn a_failing_block_rolls_back_its_work() {
        let Some(url) = support::database_url("PgPool::transaction rollback") else {
            return;
        };
        let pool = pool(&url);
        reset(&pool, "rurqe_tx_rollback");

        let rejected = pool.transaction(|tx| {
            tx.client_mut()
                .batch_execute("INSERT INTO rurqe_tx_rollback (id, amount) VALUES (1, 100)")?;
            // The statement fails inside the transaction, so the block returns
            // its error and the guard rolls the insert back.
            tx.client_mut()
                .batch_execute("INSERT INTO rurqe_tx_absent (id) VALUES (1)")?;
            Ok(())
        });
        assert!(
            matches!(&rejected, Err(PgPoolError::Query(_))),
            "the block reports the driver failure: {rejected:?}"
        );

        assert_eq!(
            totals(&pool, "rurqe_tx_rollback"),
            (0, 0),
            "the rolled back insert is gone"
        );
    }

    #[test]
    fn a_panicking_block_rolls_back_its_work() {
        let Some(url) = support::database_url("PgPool::transaction rollback on panic") else {
            return;
        };
        let pool = pool(&url);
        reset(&pool, "rurqe_tx_panic");

        let unwound = catch_unwind(AssertUnwindSafe(|| {
            pool.transaction(|tx| -> Result<(), PgError> {
                tx.client_mut()
                    .batch_execute("INSERT INTO rurqe_tx_panic (id, amount) VALUES (1, 100)")?;
                panic!("the block unwinds after inserting its row");
            })
        }));
        assert!(unwound.is_err(), "the block unwinds");

        assert_eq!(
            totals(&pool, "rurqe_tx_panic"),
            (0, 0),
            "the rollback ran while the block unwound"
        );
    }

    #[test]
    fn the_connection_returns_to_the_pool_after_every_ending() {
        let Some(url) = support::database_url("PgPool::transaction connection return") else {
            return;
        };
        let pool = pool(&url);
        reset(&pool, "rurqe_tx_return");

        pool.transaction(|tx| {
            tx.client_mut()
                .batch_execute("INSERT INTO rurqe_tx_return (id, amount) VALUES (1, 100)")?;
            Ok(())
        })
        .expect("the committing block returns its connection");

        let rejected = pool.transaction(|tx| {
            tx.client_mut()
                .batch_execute("INSERT INTO rurqe_tx_return (id, amount) VALUES (2, 200)")?;
            tx.client_mut()
                .batch_execute("INSERT INTO rurqe_tx_absent (id) VALUES (2)")?;
            Ok(())
        });
        assert!(rejected.is_err(), "the failing block reports its own error");

        let unwound = catch_unwind(AssertUnwindSafe(|| {
            pool.transaction(|tx| -> Result<(), PgError> {
                tx.client_mut()
                    .batch_execute("INSERT INTO rurqe_tx_return (id, amount) VALUES (3, 300)")?;
                panic!("the block unwinds after inserting its row");
            })
        }));
        assert!(unwound.is_err(), "the panicking block unwinds");

        // Only the first insert survived all three endings, and every ending
        // returned its connection: a checkout that never came back would leave
        // the one-connection pool with nothing to serve the counting block.
        assert_eq!(totals(&pool, "rurqe_tx_return"), (1, 100));
    }
    #[test]
    fn a_leaked_transaction_is_cleared_at_checkout() {
        let Some(url) = support::database_url("PgPool::get leaked transaction reset") else {
            return;
        };
        let pool = pool(&url);
        reset(&pool, "rurqe_tx_leak");

        // The raw pooled handle is the supported escape hatch, so the leak this
        // scenario produces is the ordinary one: a `BEGIN` the caller issued
        // itself and never closed.
        {
            let mut connection = pool.get().expect("the fixture connection is checked out");
            connection
                .batch_execute("BEGIN")
                .expect("the manual transaction opens");
            connection
                .batch_execute("INSERT INTO rurqe_tx_leak (id, amount) VALUES (1, 100)")
                .expect("the leaked row is written");
            // The handle returns to the pool with its transaction still open.
        }

        pool.transaction(|tx| {
            tx.client_mut()
                .batch_execute("INSERT INTO rurqe_tx_leak (id, amount) VALUES (2, 200)")?;
            Ok(())
        })
        .expect("the block commits on the session the checkout returned to idle");

        assert_eq!(
            totals(&pool, "rurqe_tx_leak"),
            (1, 200),
            "the reset rolled the abandoned transaction back, so only the block's row persists"
        );
    }

    #[test]
    fn an_aborted_leaked_transaction_is_cleared_at_checkout() {
        let Some(url) = support::database_url("PgPool::get aborted transaction reset") else {
            return;
        };
        let pool = pool(&url);
        reset(&pool, "rurqe_tx_aborted");

        {
            let mut connection = pool.get().expect("the fixture connection is checked out");
            connection
                .batch_execute("BEGIN")
                .expect("the manual transaction opens");
            connection
                .batch_execute("INSERT INTO rurqe_tx_aborted (id, amount) VALUES (1, 100)")
                .expect("the leaked row is written");
            // The failing statement leaves the session `idle in transaction
            // (aborted)`, the second state a checkout has to repair.
            assert!(
                connection
                    .batch_execute("INSERT INTO rurqe_tx_absent (id) VALUES (1)")
                    .is_err(),
                "the statement that aborts the session reports its failure"
            );
        }

        pool.transaction(|tx| {
            tx.client_mut()
                .batch_execute("INSERT INTO rurqe_tx_aborted (id, amount) VALUES (2, 200)")?;
            Ok(())
        })
        .expect("the block commits on the session the checkout repaired");

        assert_eq!(
            totals(&pool, "rurqe_tx_aborted"),
            (1, 200),
            "the aborted transaction was discarded, so only the block's row persists"
        );
    }

    #[test]
    fn a_lost_backend_is_replaced_at_the_checkout() {
        let Some(url) = support::database_url("PgPool::get lost backend replacement") else {
            return;
        };
        let pool = pool(&url);

        let backend = {
            let mut connection = pool.get().expect("the fixture connection is checked out");
            let row = connection
                .query_one("SELECT pg_backend_pid()", &[])
                .expect("the backend reports its process id");
            // The handle returns to the pool while its session is still healthy,
            // so the pool recycles the connection instead of discarding it.
            row.get::<_, i32>(0)
        };

        let config: postgres::Config = url.parse().expect("the test URL parses");
        let mut killer = config
            .connect(NoTls)
            .expect("the second client connects to the server");
        terminate_and_await(&mut killer, backend);

        // The recycled session died with its backend. `r2d2` probes every
        // connection it hands out, the probe fails on the session without a
        // backend, and the pool discards the connection and opens a fresh
        // one: the checkout observes the loss instead of the caller, whose
        // statement runs on a healthy session.
        let mut served = pool.get().expect("the pool serves a fresh connection");
        let served = served.query_one("SELECT 1", &[]);
        assert!(
            served.is_ok(),
            "the pool replaces the connection whose backend died: {:?}",
            served.err()
        );
    }

    #[test]
    fn a_lost_backend_is_reported_by_the_callers_statement() {
        let Some(url) = support::database_url("PgPool::get unprobed lost backend") else {
            return;
        };
        let pool = unprobed_pool(&url);

        let backend = {
            let mut connection = pool.get().expect("the fixture connection is checked out");
            let row = connection
                .query_one("SELECT pg_backend_pid()", &[])
                .expect("the backend reports its process id");
            // The handle returns to the pool while its session is still healthy,
            // so the pool recycles the connection instead of discarding it.
            row.get::<_, i32>(0)
        };

        let config: postgres::Config = url.parse().expect("the test URL parses");
        let mut killer = config
            .connect(NoTls)
            .expect("the second client connects to the server");
        terminate_and_await(&mut killer, backend);

        // The checkout hands the dead session out without probing it, so the
        // loss reaches the statement of the caller: the session a query
        // addresses no longer has a backend to answer it.
        {
            let mut connection =
                pool.get().expect("the unprobed checkout hands the session out");
            let refused = connection.query_one("SELECT 1", &[]);
            assert!(
                refused.is_err(),
                "the session without a backend reports the failure of its statement"
            );
        }
    }

    /// A refused savepoint rollback poisons the connection, so a bare
    /// statement never reaches the abandoned transaction, while the next scope
    /// still reports the driver failure of the retried recovery.
    #[test]
    fn a_poisoned_connection_refuses_its_statements() {
        let Some(url) = support::database_url("PgConnection poisoned statements") else {
            return;
        };
        let pool = pool(&url);
        reset(&pool, "rurqe_tx_poisoned");

        // The termination arrives from another session, because the session
        // that holds the savepoint is exactly the one the scenario has to lose.
        let config: postgres::Config = url.parse().expect("the test URL parses");
        let mut killer = config
            .connect(NoTls)
            .expect("the second client connects to the server");

        let outcome = pool.transaction(|tx| {
            let backend = tx
                .client_mut()
                .query_one("SELECT pg_backend_pid()", &[])?
                .get::<_, i32>(0);
            tx.client_mut().batch_execute(
                "INSERT INTO rurqe_tx_poisoned (id, amount) VALUES (1, 100)",
            )?;

            // The nested block loses its session, so the `ROLLBACK TO SAVEPOINT`
            // its guard issues cannot reach a server: the refused statement
            // poisons the connection.
            let nested: Result<(), PgError> = tx.transaction(|nested| {
                nested.client_mut().batch_execute(
                    "INSERT INTO rurqe_tx_poisoned (id, amount) VALUES (2, 200)",
                )?;
                let terminated = killer.query_one("SELECT pg_terminate_backend($1)", &[&backend])?;
                assert!(
                    terminated.get::<_, bool>(0),
                    "the server accepted the termination of backend {backend}"
                );
                Ok(())
            });
            assert!(
                matches!(&nested, Err(PgError::Driver(_))),
                "the nested block reports the savepoint release it lost: {nested:?}"
            );

            // The adapter reports the state it records, so the caller holding the
            // connection learns that the abandoned savepoint is still pending
            // rather than inferring it from the refusal of the next statement.
            assert!(
                tx.is_poisoned(),
                "the refused savepoint rollback marks the connection poisoned"
            );

            // A bare statement never runs inside the abandoned transaction: the
            // record refuses it before it reaches the driver.
            let statement = BuiltQuery {
                sql: String::from("SELECT 1"),
                params: Vec::new(),
            };
            let refused = tx.execute_raw(&statement);
            assert!(
                matches!(&refused, Err(PgError::ConnectionPoisoned { .. })),
                "the poisoned connection refuses its statements: {refused:?}"
            );

            // The recovery a poisoned connection offers is the next transaction
            // entry: it retries the abandoned savepoint rollback, and the retry
            // reports the driver failure of the session that is gone.
            let recovered: Result<(), PgError> = tx.transaction(|_| Ok(()));
            assert!(
                matches!(&recovered, Err(PgError::Driver(_))),
                "the retried savepoint recovery reports the driver failure: {recovered:?}"
            );

            Ok::<(), PgError>(())
        });

        assert!(
            matches!(&outcome, Err(PgPoolError::Query(PgError::Driver(_)))),
            "the poisoned block refuses its commit with the recorded failure: {outcome:?}"
        );
        assert_eq!(
            totals(&pool, "rurqe_tx_poisoned"),
            (0, 0),
            "the abandoned transaction was discarded with the session"
        );
    }
}

#[cfg(feature = "deadpool-postgres")]
mod async_pool {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    use deadpool_postgres::{Manager, ManagerConfig, Pool, Runtime};
    use rurqe_postgres::{PgAsyncPool, PgError};
    use tokio_postgres::NoTls;

    use crate::support;

    /// Builds a one-connection pool over the server the scenario names.
    fn pool(url: &str) -> PgAsyncPool {
        let config: tokio_postgres::Config = url.parse().expect("the test URL parses");
        let manager = Manager::from_config(config, NoTls, ManagerConfig::default());
        let pool = Pool::builder(manager)
            .max_size(1)
            .runtime(Runtime::Tokio1)
            .build()
            .expect("a one-connection pool builds");
        PgAsyncPool::new(pool)
    }

    /// Resets `table` to an empty fixture through the pool.
    async fn reset(pool: &PgAsyncPool, table: &str) {
        // The block owns the name it works with: a borrowed argument would tie
        // the boxed future of the closure to the caller's lifetime, and the
        // closure could no longer be generic over the lifetime of its scope.
        let table = table.to_owned();
        pool.transaction(|scope| {
            Box::pin(async move {
                support::execute(&scope, &format!("DROP TABLE IF EXISTS {table}")).await?;
                support::execute(
                    &scope,
                    &format!(
                        "CREATE TABLE {table} (id INTEGER PRIMARY KEY, amount INTEGER NOT NULL)"
                    ),
                )
                .await?;
                Ok::<(), PgError>(())
            })
        })
        .await
        .expect("the fixture block commits");
    }

    #[tokio::test]
    async fn a_committed_block_persists_its_work() {
        let Some(url) = support::database_url("PgAsyncPool::transaction commit") else {
            return;
        };
        let pool = pool(&url);
        reset(&pool, "rurqe_tx_async_commit").await;

        pool.transaction(|scope| {
            Box::pin(async move {
                support::execute(
                    &scope,
                    "INSERT INTO rurqe_tx_async_commit (id, amount) VALUES (1, 100)",
                )
                .await?;
                Ok::<(), PgError>(())
            })
        })
        .await
        .expect("the block commits");

        assert_eq!(
            support::totals(&pool, "rurqe_tx_async_commit").await,
            (1, 100)
        );
    }

    #[tokio::test]
    async fn a_failing_block_is_rolled_back_before_the_error_returns() {
        let Some(url) = support::database_url("PgAsyncPool::transaction rollback") else {
            return;
        };
        let pool = pool(&url);
        reset(&pool, "rurqe_tx_async_rollback").await;

        let rejected = pool
            .transaction(|scope| {
                Box::pin(async move {
                    support::execute(
                        &scope,
                        "INSERT INTO rurqe_tx_async_rollback (id, amount) VALUES (1, 100)",
                    )
                    .await?;
                    // The statement fails inside the transaction, so the closure
                    // returns its error and the adapter awaits the rollback
                    // before that error reaches this caller.
                    support::execute(&scope, "INSERT INTO rurqe_tx_absent (id) VALUES (1)")
                        .await?;
                    Ok::<(), PgError>(())
                })
            })
            .await;
        assert!(rejected.is_err(), "the block reports the driver failure");

        // The rollback was awaited, so the insert is already gone when the next
        // block reads the table.
        assert_eq!(
            support::totals(&pool, "rurqe_tx_async_rollback").await,
            (0, 0)
        );
    }
    /// Polls the yield loop grants the cancelled block before it fails.
    ///
    /// The block raises its flag from the server's acknowledgement of the
    /// insert and then parks, so a live server reaches the flag in a handful of
    /// polls; the budget exists only so a block that never gets there fails the
    /// scenario with a diagnostic instead of hanging the suite.
    const YIELD_BUDGET: usize = 100_000;

    #[tokio::test]
    async fn a_cancelled_block_does_not_leak_its_transaction() {
        let Some(url) = support::database_url("PgAsyncPool::transaction cancellation") else {
            return;
        };
        let pool = pool(&url);
        reset(&pool, "rurqe_tx_async_cancel").await;

        let inserted = Arc::new(AtomicBool::new(false));
        let block = tokio::spawn({
            let pool = pool.clone();
            let inserted = Arc::clone(&inserted);
            async move {
                pool.transaction(|scope| {
                    Box::pin(async move {
                        support::execute(
                            &scope,
                            "INSERT INTO rurqe_tx_async_cancel (id, amount) VALUES (1, 100)",
                        )
                        .await?;
                        inserted.store(true, Ordering::SeqCst);
                        // The block parks on a future that never becomes ready,
                        // so the only way out of it is the cancellation below.
                        std::future::pending::<()>().await;
                        Ok::<(), PgError>(())
                    })
                })
                .await
            }
        });

        // The flag is raised from the server's acknowledgement of the insert, so
        // the yield loop gives the block the polls it needs to reach it. The loop
        // is bounded so a block that never completes its insert fails the
        // scenario with a diagnostic instead of hanging the suite.
        let mut yields = 0;
        while !inserted.load(Ordering::SeqCst) {
            assert!(
                yields < YIELD_BUDGET,
                "the cancelled block did not complete its insert within {YIELD_BUDGET} yields"
            );
            yields += 1;
            tokio::task::yield_now().await;
        }

        // Aborting drops the block future inside the closure: neither `COMMIT`
        // nor `ROLLBACK` is ever awaited for it, so the disposition guard alone
        // decides what happens to the connection that opened the transaction.
        block.abort();
        match block.await {
            Ok(_) => panic!("the cancelled block never returns a value"),
            Err(failure) => assert!(
                failure.is_cancelled(),
                "the block was cancelled rather than failed: {failure}"
            ),
        }

        pool.transaction(|scope| {
            Box::pin(async move {
                support::execute(
                    &scope,
                    "INSERT INTO rurqe_tx_async_cancel (id, amount) VALUES (2, 200)",
                )
                .await?;
                Ok::<(), PgError>(())
            })
        })
        .await
        .expect("the pool serves a fresh connection after the cancelled block");

        assert_eq!(
            support::totals(&pool, "rurqe_tx_async_cancel").await,
            (1, 200),
            "the abandoned checkout was destroyed, so only the second block's row persisted"
        );
    }

    #[tokio::test]
    async fn a_refused_pool_rollback_poisons_and_is_reported() {
        let Some(url) = support::database_url("PgAsyncPool::transaction refused rollback") else {
            return;
        };
        let pool = pool(&url);
        reset(&pool, "rurqe_tx_async_refused").await;

        let outcome: Result<(), PgError> = pool
            .transaction(|scope| {
                Box::pin(async move {
                    // The statement ends the session: the backend terminates
                    // itself, so the `ROLLBACK` the adapter awaits below reaches
                    // a connection that no longer exists.
                    let _ =
                        support::execute(&scope, "SELECT pg_terminate_backend(pg_backend_pid())")
                            .await;
                    Err(support::deliberate_failure())
                })
            })
            .await;

        assert!(
            matches!(&outcome, Err(PgError::ConnectionPoisoned { .. })),
            "the refused rollback supersedes the block's error: {outcome:?}"
        );

        // The poisoned checkout was destroyed rather than recycled, so the pool
        // opens a fresh connection for the block that follows.
        pool.transaction(|scope| {
            Box::pin(async move {
                support::execute(
                    &scope,
                    "INSERT INTO rurqe_tx_async_refused (id, amount) VALUES (1, 100)",
                )
                .await?;
                Ok::<(), PgError>(())
            })
        })
        .await
        .expect("the pool opens a fresh connection for the next block");

        assert_eq!(
            support::totals(&pool, "rurqe_tx_async_refused").await,
            (1, 100),
            "the abandoned checkout was destroyed, so the pool serves a fresh connection"
        );
    }

    /// A block whose connection a refused savepoint rollback poisoned is not
    /// committed: the closure swallows the nested error and reports success, so
    /// the poisoned record is the only guard left before the commit.
    #[tokio::test]
    async fn a_poisoned_block_refuses_its_commit() {
        let Some(url) = support::database_url("PgAsyncPool::transaction poisoned commit") else {
            return;
        };
        let pool = pool(&url);
        reset(&pool, "rurqe_tx_async_poisoned").await;

        let outcome: Result<(), PgError> = pool
            .transaction(|mut scope| {
                Box::pin(async move {
                    support::execute(
                        &scope,
                        "INSERT INTO rurqe_tx_async_poisoned (id, amount) VALUES (1, 100)",
                    )
                    .await?;

                    let nested: Result<(), PgError> = scope
                        .transaction(|nested| {
                            Box::pin(async move {
                                // The statement ends the session, so the
                                // `ROLLBACK TO SAVEPOINT` the adapter awaits for
                                // the nested block reaches a connection that no
                                // longer exists.
                                let _ = support::execute(
                                    &nested,
                                    "SELECT pg_terminate_backend(pg_backend_pid())",
                                )
                                .await;
                                Err(support::deliberate_failure())
                            })
                        })
                        .await;
                    assert!(
                        nested.is_err(),
                        "the nested block reports the failure it returned"
                    );

                    // The enclosing block swallows the nested failure and
                    // reports success, which is the ending the commit has to
                    // refuse on its own.
                    Ok::<(), PgError>(())
                })
            })
            .await;

        match &outcome {
            Err(PgError::ConnectionPoisoned { reason }) => assert!(
                reason.contains("ROLLBACK TO SAVEPOINT"),
                "the reason names the savepoint rollback that failed: {reason}"
            ),
            other => panic!("the poisoned block refuses its commit: {other:?}"),
        }

        // The destroyed checkout left the uncommitted insert behind, and the pool
        // serves a fresh connection for the next block.
        assert_eq!(
            support::totals(&pool, "rurqe_tx_async_poisoned").await,
            (0, 0)
        );
    }
}

#[cfg(feature = "tokio-postgres")]
mod async_connection {
    use std::any::Any;
    use std::future::Future;
    use std::panic::{catch_unwind, AssertUnwindSafe};
    use std::pin::pin;
    use std::task::Poll;

    use rurqe_postgres::rurqe_core::query::{AsyncQueryBackend, IsolationLevel};
    use rurqe_postgres::{PgAsyncConnection, PgError};
    use tokio_postgres::NoTls;

    use crate::support;

    /// Connects to the server the scenario names and drives its socket.
    async fn connect(url: &str) -> PgAsyncConnection {
        let (client, connection) = tokio_postgres::connect(url, NoTls)
            .await
            .expect("the test server accepts the connection");
        // The task drives the socket until the client is dropped, which is what
        // turns the client handle into the adapter the scenarios drive.
        tokio::spawn(async move {
            let _ = connection.await;
        });
        PgAsyncConnection::new(client)
    }

    #[tokio::test]
    async fn a_scope_runs_under_the_requested_isolation_level() {
        let Some(url) = support::database_url("PgAsyncConnection::transaction_with_isolation")
        else {
            return;
        };
        let mut connection = connect(&url).await;

        let isolation = connection
            .transaction_with_isolation(IsolationLevel::Serializable, |scope| {
                Box::pin(async move {
                    let sql = "SELECT current_setting('transaction_isolation')";
                    let rows =
                        AsyncQueryBackend::fetch_raw(&scope, &support::statement(sql)).await?;
                    support::single_text(rows, sql)
                })
            })
            .await
            .expect("the serializable block commits");

        assert_eq!(
            isolation, "serializable",
            "the block runs at the isolation level it asked for"
        );
    }

    #[tokio::test]
    async fn a_nested_scope_rolls_back_only_itself() {
        let Some(url) = support::database_url("PgAsyncScope::transaction savepoint nesting") else {
            return;
        };
        let mut connection = connect(&url).await;
        let table = "rurqe_tx_async_nested";

        connection
            .transaction_with_isolation(IsolationLevel::Serializable, |mut scope| {
                Box::pin(async move {
                    support::execute(&scope, &format!("DROP TABLE IF EXISTS {table}")).await?;
                    support::execute(
                        &scope,
                        &format!(
                            "CREATE TABLE {table} (id INTEGER PRIMARY KEY, amount INTEGER NOT NULL)"
                        ),
                    )
                    .await?;
                    support::execute(
                        &scope,
                        &format!("INSERT INTO {table} (id, amount) VALUES (1, 100)"),
                    )
                    .await?;

                    let nested = scope
                        .transaction(|nested| {
                            Box::pin(async move {
                                support::execute(
                                    &nested,
                                    &format!("INSERT INTO {table} (id, amount) VALUES (2, 200)"),
                                )
                                .await?;
                                // The statement fails inside the savepoint, so
                                // the nested block rolls back to it and discards
                                // its own insert alone.
                                support::execute(
                                    &nested,
                                    "INSERT INTO rurqe_tx_absent (id) VALUES (2)",
                                )
                                .await?;
                                Ok::<(), PgError>(())
                            })
                        })
                        .await;
                    assert!(nested.is_err(), "the nested block reports its own failure");

                    support::execute(
                        &scope,
                        &format!("INSERT INTO {table} (id, amount) VALUES (3, 300)"),
                    )
                    .await?;
                    Ok::<(), PgError>(())
                })
            })
            .await
            .expect("the enclosing block commits");

        // The savepoint undid the nested insert alone, so the work written
        // around it committed: the enclosing block and the row after it.
        assert_eq!(support::totals(&connection, table).await, (2, 400));
    }
    /// The outcome of a block driven through [`catch_unwind_future`].
    type BlockOutcome<T> = Result<Result<T, PgError>, Box<dyn Any + Send>>;

    /// Drives `future` to completion, capturing a panic instead of unwinding.
    ///
    /// An unwinding block still has to be observed by the exit guard of the
    /// adapter, so the panic is caught at the poll boundary: the unwind leaves
    /// the poll call, the future is dropped through the ordinary drop path on
    /// the way out, and the guard marks the connection poisoned exactly as it
    /// does when a block returns.
    async fn catch_unwind_future<T, F>(future: F) -> BlockOutcome<T>
    where
        F: Future<Output = Result<T, PgError>>,
    {
        let mut future = pin!(future);
        std::future::poll_fn(move |context| {
            match catch_unwind(AssertUnwindSafe(|| future.as_mut().poll(context))) {
                Ok(Poll::Pending) => Poll::Pending,
                Ok(Poll::Ready(output)) => Poll::Ready(Ok(output)),
                Err(payload) => Poll::Ready(Err(payload)),
            }
        })
        .await
    }

    #[tokio::test]
    async fn a_panicking_nested_block_poisons_the_connection() {
        let Some(url) = support::database_url("PgAsyncConnection panicking nested block") else {
            return;
        };
        let mut connection = connect(&url).await;
        let table = "rurqe_tx_async_panic";

        let outcome: BlockOutcome<()> = catch_unwind_future(
            connection.transaction_with_isolation(IsolationLevel::Serializable, |mut scope| {
                Box::pin(async move {
                    support::execute(&scope, &format!("DROP TABLE IF EXISTS {table}")).await?;
                    support::execute(
                        &scope,
                        &format!(
                            "CREATE TABLE {table} (id INTEGER PRIMARY KEY, amount INTEGER NOT NULL)"
                        ),
                    )
                    .await?;
                    support::execute(
                        &scope,
                        &format!("INSERT INTO {table} (id, amount) VALUES (1, 100)"),
                    )
                    .await?;

                    // The nested block is a savepoint of the enclosing
                    // transaction, and its panic unwinds through both scopes:
                    // neither block ever issues a control statement.
                    let _nested: Result<(), PgError> = scope
                        .transaction(|nested| {
                            Box::pin(async move {
                                support::execute(
                                    &nested,
                                    &format!("INSERT INTO {table} (id, amount) VALUES (2, 200)"),
                                )
                                .await?;
                                panic!("the nested block unwinds after inserting its row");
                            })
                        })
                        .await;
                    Ok::<(), PgError>(())
                })
            }),
        )
        .await;

        let payload = outcome.expect_err("the wrapper captures the panic the block unwinds with");
        let message = payload
            .downcast_ref::<&str>()
            .map(|text| (*text).to_owned())
            .or_else(|| payload.downcast_ref::<String>().cloned());
        assert_eq!(
            message.as_deref(),
            Some("the nested block unwinds after inserting its row"),
            "the panic that unwound through the block is the nested block's own"
        );

        assert!(
            connection.is_poisoned(),
            "the block unwound without closing its transaction, so the connection is poisoned"
        );

        let refused =
            AsyncQueryBackend::fetch_raw(&connection, &support::statement("SELECT 1")).await;
        assert!(
            matches!(&refused, Err(PgError::ConnectionPoisoned { .. })),
            "the poisoned connection refuses further statements: {refused:?}"
        );
    }

    #[tokio::test]
    async fn a_refused_rollback_poisons_and_is_reported() {
        let Some(url) = support::database_url("PgAsyncConnection refused rollback") else {
            return;
        };
        let mut connection = connect(&url).await;

        let outcome: Result<(), PgError> = connection
            .transaction_with_isolation(IsolationLevel::ReadCommitted, |scope| {
                Box::pin(async move {
                    // The statement ends the session: the backend terminates
                    // itself, so the `ROLLBACK` the adapter awaits below reaches
                    // a connection that no longer exists.
                    let _ =
                        support::execute(&scope, "SELECT pg_terminate_backend(pg_backend_pid())")
                            .await;
                    Err(support::deliberate_failure())
                })
            })
            .await;

        match &outcome {
            Err(PgError::InvalidValue { reason, .. }) => assert_eq!(
                reason.as_str(),
                "the block failed before its rollback",
                "the block reports the failure it returned"
            ),
            other => panic!("the refused rollback supersedes the block's error: {other:?}"),
        }

        assert!(
            connection.is_poisoned(),
            "the refused rollback recorded a poison on the connection"
        );

        let refused =
            AsyncQueryBackend::fetch_raw(&connection, &support::statement("SELECT 1")).await;
        assert!(
            matches!(&refused, Err(PgError::ConnectionPoisoned { .. })),
            "the poisoned connection refuses further statements: {refused:?}"
        );
    }
    /// A handle dropped without a terminal call leaves its server-side
    /// transaction open, and the adapter proves nothing about it: the connection
    /// is poisoned, so the next caller is refused instead of running inside the
    /// abandoned transaction.
    #[tokio::test]
    async fn a_dropped_handle_poisons_the_connection() {
        let Some(url) = support::database_url("PgAsyncTransaction dropped handle") else {
            return;
        };
        let mut connection = connect(&url).await;

        {
            let handle = connection
                .transaction()
                .await
                .expect("the transaction opens");
            support::execute(&handle, "SELECT 1")
                .await
                .expect("the statement runs inside the transaction");
            // The handle reaches its `Drop` with the transaction still open: the
            // rollback the driver requests from there is a fire-and-forget
            // statement, so the adapter can prove nothing about the transaction
            // the server still holds.
        }

        assert!(
            connection.is_poisoned(),
            "the dropped handle records the abandoned transaction on the connection"
        );

        let refused =
            AsyncQueryBackend::fetch_raw(&connection, &support::statement("SELECT 1")).await;
        assert!(
            matches!(&refused, Err(PgError::ConnectionPoisoned { .. })),
            "the poisoned connection refuses further statements: {refused:?}"
        );
    }

    /// A terminal call whose future is dropped after the handle handed its
    /// transaction over proves nothing: the `COMMIT` may never have reached the
    /// server, so the connection is poisoned instead of being handed to the next
    /// caller inside a transaction nothing can close.
    #[tokio::test]
    async fn a_dropped_commit_poisons_the_connection() {
        let Some(url) = support::database_url("PgAsyncTransaction dropped commit") else {
            return;
        };
        let mut connection = connect(&url).await;

        let handle = connection
            .transaction()
            .await
            .expect("the transaction opens");
        support::execute(&handle, "SELECT 1")
            .await
            .expect("the statement runs inside the transaction");
        // The first poll takes the transaction out of the handle and dispatches
        // the `COMMIT`, and the drop that follows leaves the statement without
        // its answer.
        poll_once_then_drop(handle.commit()).await;

        assert!(
            connection.is_poisoned(),
            "the dropped terminal call records the unproven transaction on the connection"
        );

        let refused =
            AsyncQueryBackend::fetch_raw(&connection, &support::statement("SELECT 1")).await;
        assert!(
            matches!(&refused, Err(PgError::ConnectionPoisoned { .. })),
            "the poisoned connection refuses further statements: {refused:?}"
        );
    }
    /// The rollback form is guarded the same way: a dropped `ROLLBACK` leaves the
    /// statements of the transaction pending on the server, so the connection is
    /// poisoned as well.
    #[tokio::test]
    async fn a_dropped_rollback_poisons_the_connection() {
        let Some(url) = support::database_url("PgAsyncTransaction dropped rollback") else {
            return;
        };
        let mut connection = connect(&url).await;

        let handle = connection
            .transaction()
            .await
            .expect("the transaction opens");
        support::execute(&handle, "SELECT 1")
            .await
            .expect("the statement runs inside the transaction");
        // The terminal call is dropped before the driver answers the `ROLLBACK`,
        // so the adapter cannot prove the transaction was closed.
        poll_once_then_drop(handle.rollback()).await;

        assert!(
            connection.is_poisoned(),
            "the dropped terminal call records the unproven transaction on the connection"
        );

        let refused =
            AsyncQueryBackend::fetch_raw(&connection, &support::statement("SELECT 1")).await;
        assert!(
            matches!(&refused, Err(PgError::ConnectionPoisoned { .. })),
            "the poisoned connection refuses further statements: {refused:?}"
        );
    }
    /// Polls `future` once and drops it, leaving the statement it dispatched in
    /// flight.
    ///
    /// The first poll is what sends the `BEGIN` to the server, and the drop that
    /// follows is the ending a caller produces when it cancels the entry: the
    /// response never arrives, so the connection cannot learn whether the
    /// transaction opened.
    async fn poll_once_then_drop<F: Future>(future: F) {
        let mut future = pin!(future);
        std::future::poll_fn(move |context| {
            let _ = future.as_mut().poll(context);
            Poll::Ready(())
        })
        .await;
    }

    #[tokio::test]
    async fn a_refused_entry_poisons_the_connection() {
        let Some(url) = support::database_url("PgAsyncConnection refused begin") else {
            return;
        };
        let mut connection = connect(&url).await;

        // The session ends before the transaction below opens, so the `BEGIN`
        // reaches a connection that no longer exists. The probe waits for the
        // client to observe the end of the session instead of racing it.
        let termination = "SELECT pg_terminate_backend(pg_backend_pid())";
        let _ = support::execute(&connection, termination).await;
        let query = support::statement("SELECT 1");
        let probe = AsyncQueryBackend::fetch_raw(&connection, &query).await;
        assert!(probe.is_err(), "the ended session refuses the probe");

        let failed = match connection.transaction().await {
            Ok(_) => panic!("the `BEGIN` of an ended session is refused"),
            Err(failure) => failure,
        };

        assert!(
            connection.is_poisoned(),
            "the refused entry records the failure on the connection: {failed}"
        );

        let refused = AsyncQueryBackend::fetch_raw(&connection, &query).await;
        match refused {
            Err(PgError::ConnectionPoisoned { reason }) => assert!(
                reason.contains("BEGIN"),
                "the reason names the statement that failed: {reason}"
            ),
            other => panic!("the poisoned connection refuses further statements: {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_dropped_entry_poisons_the_connection() {
        let Some(url) = support::database_url("PgAsyncConnection dropped transaction entry") else {
            return;
        };
        let mut connection = connect(&url).await;

        // The `BEGIN` is dispatched by the first poll and the future is dropped
        // before the response arrives, so the caller never learns whether the
        // transaction opened.
        poll_once_then_drop(connection.transaction()).await;

        assert!(
            connection.is_poisoned(),
            "the abandoned entry records the open transaction on the connection"
        );

        let query = support::statement("SELECT 1");
        let refused = AsyncQueryBackend::fetch_raw(&connection, &query).await;
        assert!(
            matches!(&refused, Err(PgError::ConnectionPoisoned { .. })),
            "the poisoned connection refuses further statements: {refused:?}"
        );
    }

    /// A savepoint rollback the driver refuses poisons the connection, and the
    /// enclosing block must not commit over it: the closure swallows the nested
    /// error and returns `Ok`, so the record is the only guard left.
    #[tokio::test]
    async fn a_poisoned_block_refuses_its_commit() {
        let Some(url) = support::database_url("PgAsyncConnection poisoned commit") else {
            return;
        };
        let mut connection = connect(&url).await;

        let outcome: Result<(), PgError> = connection
            .transaction_with_isolation(IsolationLevel::ReadCommitted, |mut scope| {
                Box::pin(async move {
                    let nested: Result<(), PgError> = scope
                        .transaction(|nested| {
                            Box::pin(async move {
                                // The statement ends the session, so the
                                // `ROLLBACK TO SAVEPOINT` the adapter awaits for
                                // the nested block reaches a connection that no
                                // longer exists.
                                let _ = support::execute(
                                    &nested,
                                    "SELECT pg_terminate_backend(pg_backend_pid())",
                                )
                                .await;
                                Err(support::deliberate_failure())
                            })
                        })
                        .await;
                    assert!(
                        nested.is_err(),
                        "the nested block reports the failure it returned"
                    );

                    Ok::<(), PgError>(())
                })
            })
            .await;

        match &outcome {
            Err(PgError::ConnectionPoisoned { reason }) => assert!(
                reason.contains("ROLLBACK TO SAVEPOINT"),
                "the reason names the savepoint rollback that failed: {reason}"
            ),
            other => panic!("the poisoned block refuses its commit: {other:?}"),
        }

        assert!(
            connection.is_poisoned(),
            "the connection stays poisoned after the refused commit"
        );
    }
}
