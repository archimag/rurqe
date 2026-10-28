//! Aggregation: `GROUP BY`, aggregate projections, `HAVING`, and
//! `FILTER (WHERE ...)`.

use rurqe::postgres::prelude::*;

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

fn main() {
    // ANCHOR: group_by
    let per_user = select_from(User)
        .inner_join(Order, OrderUserId.eq(UserId))
        .group_by(UserId)
        .select(select! {
            user_id: UserId,
            orders: count_all(),
        })
        .having(count_all().gt(0i64))
        .build()
        .expect("valid aggregate query");
    // ANCHOR_END: group_by
    println!("{}", per_user.sql);

    // ANCHOR: filtered
    let refunded = select_from(Order)
        .select((sum(OrderTotal).filter_where(OrderStatus.eq("refunded")),))
        .build()
        .expect("valid filtered aggregate");
    // ANCHOR_END: filtered
    println!("{}", refunded.sql);
}
