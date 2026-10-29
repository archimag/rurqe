//! The shortest complete walkthrough: declare a table, build a typed query,
//! and execute it against an asynchronous pool.
//!
//! Examples compile without a database: constructing a pool opens no
//! connection, so `main` is compiled but never executed by the verification
//! pipeline.

use deadpool_postgres::Runtime;
use rurqe::postgres::prelude::*;
use tokio_postgres::NoTls;

table! {
    /// The `users` table.
    User => "users" {
        UserId => "id": i64,
        UserName => "name": String,
        UserEmail => "email": String,
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let pool = example_pool();

    // ANCHOR: query
    let users = select_from(User)
        .select(cols!(UserId, UserName, UserEmail))
        .where_(Expr::column::<UserId>().eq(7i64))
        .fetch_all(&pool)
        .await?;
    // ANCHOR_END: query

    // ANCHOR: build_refusal
    // Syntactically sound but semantically invalid: `DISTINCT ON` deduplicates
    // the rows on one expression while the leading `ORDER BY` criterion names
    // another, so no engine would accept the statement.
    let rejected = select_from(User)
        .select(cols!(UserId, UserName))
        .distinct_on(UserName)
        .order_by_asc(UserId)
        .build();
    assert!(rejected.is_err());
    // ANCHOR_END: build_refusal
    println!("{}", users.len());
    Ok(())
}

/// Builds the asynchronous pool every example executes against.
///
/// The pool is lazy — it opens no connection until the first statement is sent
/// — so an example compiles and runs without a reachable database.
fn example_pool() -> PgAsyncPool {
    // The configuration is a literal, so building the pool fails only for a
    // reason the example itself cannot cause; the pool still opens no
    // connection until the first statement is sent.
    let pool = deadpool_postgres::Config::new()
        .create_pool(Some(Runtime::Tokio1), NoTls)
        .expect("the example pool configuration is valid");
    PgAsyncPool::new(pool)
}
