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
}

#[cfg(feature = "r2d2")]
mod sync_pool {
    use std::panic::{catch_unwind, AssertUnwindSafe};

    use postgres::NoTls;
    use r2d2_postgres::PostgresConnectionManager;
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
}

#[cfg(feature = "deadpool-postgres")]
mod async_pool {
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
}

#[cfg(feature = "tokio-postgres")]
mod async_connection {
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
}
