//! Schema declaration and the two spellings of column addressing.

use deadpool_postgres::Runtime;
use rurqe::postgres::prelude::*;
use tokio_postgres::NoTls;

// ANCHOR: declaration
table! {
    /// The `users` table.
    User => "users" {
        UserId => "id": i64,
        UserName => "name": String,
        UserActive => "active": bool,
    }
}
// ANCHOR_END: declaration

#[tokio::main]
async fn main() -> Result<()> {
    let pool = example_pool();

// ANCHOR: addressing
    // The bare marker: the statement reads one relation, so the column needs no
    // qualification.
    let by_bare_marker = select_from(User)
        .select(cols!(UserId))
        .where_(UserId.eq(1i64))
        .fetch_all(&pool)
        .await?;
    // The table-prefixed marker is the same marker, spelled through the table
    // that owns it.
    let by_table_prefix = select_from(User)
        .select(cols!(User.UserId))
        .where_(User.UserId.eq(1i64))
        .fetch_all(&pool)
        .await?;
// ANCHOR_END: addressing
    println!("{} {}", by_bare_marker.len(), by_table_prefix.len());

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
