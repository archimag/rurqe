//! Combining statements: `UNION`, `INTERSECT`, `EXCEPT`.

use deadpool_postgres::Runtime;
use rurqe::postgres::prelude::*;
use tokio_postgres::NoTls;

table! {
    /// The `users` table.
    User => "users" {
        UserId => "id": i64,
        UserName => "name": String,
    }
}

table! {
    /// The `archived_users` table.
    ArchivedUser => "archived_users" {
        ArchivedUserId => "id": i64,
        ArchivedUserName => "name": String,
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let pool = example_pool();

    // ANCHOR: union
    let combined = select_from(User)
        .select((UserName,))
        .union_all(select_from(ArchivedUser).select((ArchivedUserName,)))
        .fetch_all(&pool)
        .await?;
    // ANCHOR_END: union
    println!("{}", combined.len());

    // ANCHOR: ordered
    let page = select_from(User)
        .select((UserName,))
        .union_all(select_from(ArchivedUser).select((ArchivedUserName,)))
        .order_by_output_asc(UserName)
        .limit(50)
        .fetch_all(&pool)
        .await?;
    // ANCHOR_END: ordered
    println!("{}", page.len());

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
