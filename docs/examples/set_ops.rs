//! Combining statements: `UNION`, `INTERSECT`, `EXCEPT`.

use rurqe::postgres::prelude::*;
use rurqe_examples::example_pool;
table!(
    /// The `users` table.
    User => "users" {
        UserId => "id": i64,
        UserName => "name": String,
    }
);

table!(
    /// The `archived_users` table.
    ArchivedUser => "archived_users" {
        ArchivedUserId => "id": i64,
        ArchivedUserName => "name": String,
    }
);

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

