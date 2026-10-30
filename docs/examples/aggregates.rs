//! Aggregation: `GROUP BY`, aggregate projections, `HAVING`, and
//! `FILTER (WHERE ...)`.

use deadpool_postgres::Runtime;
use rurqe::postgres::prelude::*;
use tokio_postgres::NoTls;

table! {
    /// The `users` table.
    User => "users" {
        UserId => "id": i64,
    }
}

table! {
    /// The `orders` table.
    Order => "orders" {
        OrderId => "id": i64,
        OrderTotal => "total": f64,
        OrderStatus => "status": String,
        OrderUserId => "user_id": i64,
    }
}

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
