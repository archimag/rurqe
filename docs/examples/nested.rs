//! Typed nested collections from a single round trip: `cols!` with
//! `nest_many` and `nest_one`, so the children of every parent arrive as one
//! JSON document.

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
    /// The `orders` table.
    Order => "orders" {
        OrderId => "id": i64,
        OrderTotal => "total": f64,
        OrderUserId => "user_id": i64,
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let pool = example_pool();
// ANCHOR: nested
    let users_with_orders = select_from(User)
        .select(cols! {
            user_name: UserName,
            orders: nest_many(
                select_from(Order)
                    .select(cols! {
                        order_id: OrderId,
                        total: OrderTotal,
                    })
                    .where_(OrderUserId.eq(UserId.without_scope()))
                    .order_by_desc(OrderTotal),
            ),
        })
        .fetch_all(&pool)
        .await?;
// ANCHOR_END: nested
    println!("{}", users_with_orders.len());

// ANCHOR: nest_one
    // The same shape for a one-to-one relation: the collection stops at the
    // first row, so the field reads as an `Option` of the nested projection
    // and an absent child stays distinguishable from a child without values.
    let users_with_top_order = select_from(User)
        .select(cols! {
            user_name: UserName,
            top_order: nest_one(
                select_from(Order)
                    .select(cols! {
                        order_id: OrderId,
                        total: OrderTotal,
                    })
                    .where_(OrderUserId.eq(UserId.without_scope()))
                    .order_by_desc(OrderTotal),
            ),
        })
        .fetch_all(&pool)
        .await?;
// ANCHOR_END: nest_one
    println!("{}", users_with_top_order.len());

// ANCHOR: correlation
    // The child statement introduces only `orders` in its scope, so a column
    // of the enclosing query is marked as resolved elsewhere instead of being
    // looked up among the relations the child statement declares.
    let _correlated = select_from(Order)
        .select(cols!(OrderTotal))
        .where_(OrderUserId.eq(UserId.without_scope()))
        .build()
        .expect("the correlated statement is valid");
// ANCHOR_END: correlation

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
