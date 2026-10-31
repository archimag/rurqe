//! Value expressions: arithmetic and `CASE WHEN`.

use rurqe::postgres::prelude::*;
use rurqe_examples::example_pool;
table!(
    /// The `orders` table.
    Order => "orders" {
        OrderId => "id": i64,
        OrderStatus => "status": String,
        OrderTotal => "total": f64,
        OrderTax => "tax": f64,
    }
);

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

