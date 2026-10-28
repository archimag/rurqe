//! Typed nested collections from a single round trip: `dto!` with
//! `nest_many`.

use rurqe::postgres::prelude::*;

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

fn main() {
    // ANCHOR: nested
    let users_with_orders = select_from(User)
        .select(dto!(UserWithOrders {
            user_id: UserId,
            user_name: UserName,
            orders: nest_many(
                select_from(Order)
                    .select(dto!(OrderSummary {
                        order_id: OrderId,
                        total: OrderTotal,
                    }))
                    .where_(OrderUserId.eq(UserId)),
            ),
        }))
        .build()
        .expect("valid nested query");
    // ANCHOR_END: nested
    println!("{}", users_with_orders.sql);
}
