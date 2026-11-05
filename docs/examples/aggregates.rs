//! Aggregation: `GROUP BY`, aggregate projections, `HAVING`, and
//! `FILTER (WHERE ...)`.

use rurqe::postgres::prelude::*;
use rurqe_examples::example_pool;
use rurqe_examples::markers::*;

#[tokio::main]
async fn main() -> Result<()> {
    let pool = example_pool();

    // ANCHOR: group_by
    let per_user = select_from(User)
        .inner_join(Order, OrderUserId.eq(UserId))
        .group_by(UserId)
        .select(cols! {
            user_id: UserId,
            orders: count_all(),
        })
        .having(count_all().gt(0i64))
        .fetch_all(&pool)
        .await?;
    // ANCHOR_END: group_by
    println!("{}", per_user.len());

    // ANCHOR: filtered
    let refunded = select_from(Order)
        .select((sum(OrderTotal).filter_where(OrderStatus.eq("refunded")),))
        .fetch_one(&pool)
        .await?;
    // ANCHOR_END: filtered
    println!("{refunded:?}");

    // ANCHOR: having_col
    let filtered = select_from(Order)
        .group_by(OrderUserId)
        .select((OrderUserId, count_all()))
        .having_col(OrderUserId, OrderUserId.gt(10i64))
        .fetch_all(&pool)
        .await?;
    // ANCHOR_END: having_col
    println!("{}", filtered.len());

    // ANCHOR: group_ordering
    let top = select_from(Order)
        .group_by(OrderUserId)
        .select((OrderUserId, count_all()))
        .order_by_expr_desc(count_all())
        .limit(10)
        .fetch_all(&pool)
        .await?;
    // ANCHOR_END: group_ordering
    println!("{}", top.len());

    Ok(())
}
