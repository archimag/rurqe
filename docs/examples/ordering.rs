//! Ordering the result and paginating it.

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
    /// The `events` table.
    Event => "events" {
        EventId => "id": i64,
        EventCreatedAt => "created_at": i64,
        EventData => "data": String,
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let pool = example_pool();

    // ANCHOR: order_by
    let page = select_from(User)
        .select(cols!(UserId, UserName))
        .where_(UserId.gt(10i64))
        .order_by_asc(UserId)
        .limit(5)
        .offset(10)
        .fetch_all(&pool)
        .await?;
    // ANCHOR_END: order_by
    println!("{}", page.len());

    // ANCHOR: nulls
    let ordered = select_from(User)
        .select(cols!(UserName))
        .order_by_desc_nulls_last(UserName)
        .fetch_all(&pool)
        .await?;
    // ANCHOR_END: nulls
    println!("{}", ordered.len());

    // ANCHOR: window_order
    let by_window = select_from(Event)
        .select(cols!(EventId, EventCreatedAt))
        .order_by_expr_desc(row_number().over(Window::order_by_asc(EventCreatedAt)))
        .fetch_all(&pool)
        .await?;
    // ANCHOR_END: window_order
    println!("{}", by_window.len());

    // ANCHOR: keyset
    let cursor_time = 0i64;
    let cursor_id = 0i64;

    let next_page = select_from(Event)
        .select(cols!(EventId, EventCreatedAt, EventData))
        .where_((EventCreatedAt, EventId).lt((cursor_time, cursor_id)))
        .order_by_desc(EventCreatedAt)
        .order_by_desc(EventId)
        .limit(50)
        .fetch_all(&pool)
        .await?;
    // ANCHOR_END: keyset
    println!("{}", next_page.len());

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
