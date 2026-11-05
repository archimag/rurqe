//! Nested statements: scalar subqueries, `EXISTS`, and `IN (SELECT ...)`.

use rurqe::postgres::prelude::*;
use rurqe_examples::example_pool;
use rurqe_examples::markers::*;

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
                .where_(OrderUserId.eq(outer(UserId))),
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
