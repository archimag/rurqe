//! Typed nested collections from a single round trip: `cols!` with
//! `nest_many`, so the children of every parent arrive as one JSON document.

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
    // One statement reads every parent together with the rows of its
    // collection: the engine aggregates the children into a JSON document and
    // the record field decodes into a `Vec` of the nested projection, so no
    // second statement is sent and no Cartesian product is folded by hand.
    let users_with_orders = select_from(User)
        .select(cols! {
            user_name: UserName,
            orders: nest_many(
                select_from(Order)
                    .select(cols! {
                        order_id: OrderId,
                        total: OrderTotal,
                    })
                    // The child statement declares only `orders`, so the key of
                    // the enclosing row enters its predicate as a value that
                    // statement needs no relation for.
                    .where_(OrderUserId.eq(Expr::column::<UserId>().without_scope()))
                    .order_by_desc(OrderTotal),
            ),
        })
        .fetch_all(&pool)
        .await?;
    // ANCHOR_END: nested
    println!("{}", users_with_orders.len());

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
