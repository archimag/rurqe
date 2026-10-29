//! Value expressions: arithmetic and `CASE WHEN`.

use deadpool_postgres::Runtime;
use rurqe::postgres::prelude::*;
use tokio_postgres::NoTls;

table! {
    /// The `orders` table.
    Order => "orders" {
        OrderId => "id": i64,
        OrderStatus => "status": String,
        OrderTotal => "total": f64,
        OrderTax => "tax": f64,
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let pool = example_pool();

    // ANCHOR: arithmetic
    let rows = select_from(Order)
        .select(cols!(
            order_id: OrderId,
            total_with_tax: OrderTotal + OrderTax,
        ))
        .where_((OrderTotal + OrderTax).gt(200.0f64))
        .fetch_all(&pool)
        .await?;
    // ANCHOR_END: arithmetic
    println!("{}", rows.len());

    // ANCHOR: case
    let billed = select_from(Order)
        .select(cols!(
            order_id: OrderId,
            billed: Expr::<f64>::case()
                .when(OrderStatus.eq("shipped"), OrderTotal)
                .else_(OrderTax)
                .end(),
        ))
        .fetch_all(&pool)
        .await?;
    // ANCHOR_END: case
    for row in billed {
        println!("{} {}", row.order_id, row.billed);
    }

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
