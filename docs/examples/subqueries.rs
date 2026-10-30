//! Nested statements: scalar subqueries, `EXISTS`, and `IN (SELECT ...)`.

use deadpool_postgres::Runtime;
use rurqe::postgres::prelude::*;
use tokio_postgres::NoTls;

table! {
    /// The `products` table.
    Product => "products" {
        ProductId => "id": i64,
        ProductName => "name": String,
        ProductPrice => "price": f64,
    }
}

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
        OrderUserId => "user_id": i64,
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let pool = example_pool();

    // ANCHOR: scalar
    let above_average = select_from(Product)
        .select(cols!(ProductId, ProductName, ProductPrice))
        .where_(ProductPrice.gt(select_from(Product).select((avg(ProductPrice),))))
        .fetch_all(&pool)
        .await?;
    // ANCHOR_END: scalar
    println!("{}", above_average.len());

    // ANCHOR: exists
    let buyers = select_from(User)
        .select(cols!(UserId, UserName))
        .where_(Expr::exists(
            select_from(Order)
                .select((OrderUserId,))
                .where_(OrderUserId.eq(UserId.without_scope())),
        ))
        .fetch_all(&pool)
        .await?;
    // ANCHOR_END: exists
    println!("{}", buyers.len());

    // ANCHOR: in_subquery
    let with_orders = select_from(User)
        .select(cols!(UserId, UserName))
        .where_(UserId.in_subquery(select_from(Order).select((OrderUserId,))))
        .fetch_all(&pool)
        .await?;
    // ANCHOR_END: in_subquery
    println!("{}", with_orders.len());

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
