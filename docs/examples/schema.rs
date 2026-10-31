//! Schema declaration and the two spellings of column addressing.

use rurqe::postgres::prelude::*;
use rurqe_examples::example_pool;
// ANCHOR: declaration
table!(
    /// The `users` table.
    User => "users" {
        UserId => "id": i64,
        UserName => "name": String,
        UserActive => "active": bool,
    }
);
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
    println!("{}", by_bare_marker.len());
    println!("{}", by_table_prefix.len());

    Ok(())
}

