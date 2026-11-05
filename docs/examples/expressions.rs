//! Value expressions: arithmetic, `CASE WHEN`, and the null-handling functions.

use rurqe::postgres::prelude::*;
use rurqe_examples::example_pool;
table!(
    /// The `orders` table.
    Order => "orders" {
        OrderId => "id": i64,
        OrderStatus => "status": String,
        OrderTotal => "total": f64,
        OrderTax => "tax": f64,
        OrderCreatedAt => "created_at": chrono::DateTime<chrono::Utc>,
    }
);

// ANCHOR: custom_function_definition
/// Truncates a timestamp column to the day, as PostgreSQL `date_trunc` does.
fn date_trunc_day(column: OrderCreatedAt) -> Expr<chrono::DateTime<chrono::Utc>, Order> {
    Expr::<chrono::DateTime<chrono::Utc>>::fragments("date_trunc(")
        .value("day")
        .text(", ")
        .operand(column)
        .text(")")
        .end()
}
// ANCHOR_END: custom_function_definition

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

    // ANCHOR: scalar_functions
    let priced = select_from(Order)
        .select(cols!(
            order_id: OrderId,
            total: nullif(OrderTotal, 0.0),
            highest: greatest(OrderTotal, [1.0, 2.0]),
        ))
        .fetch_all(&pool)
        .await?;
    // ANCHOR_END: scalar_functions
    println!("{}", priced.len());

    // ANCHOR: null_safe
    let reclassified = select_from(Order)
        .select(cols!(order_id: OrderId))
        .where_(is_not_distinct_from(OrderTax, 0.0))
        .fetch_all(&pool)
        .await?;
    // ANCHOR_END: null_safe
    println!("{}", reclassified.len());

    // ANCHOR: period_analytics
    let recent_orders = select_from(Order)
        .select(cols!(order_id: OrderId))
        .where_(sql!(<bool> "created_at > now() - ?::interval", "30 days"))
        .fetch_all(&pool)
        .await?;
    // ANCHOR_END: period_analytics
    println!("{}", recent_orders.len());

    // ANCHOR: custom_function
    let heat_map = select_from(Order)
        .select(cols!(
            order_id: OrderId,
            day: date_trunc_day(OrderCreatedAt),
        ))
        .fetch_all(&pool)
        .await?;
    // ANCHOR_END: custom_function
    for row in heat_map {
        println!("{} {}", row.order_id, row.day);
    }

    Ok(())
}
