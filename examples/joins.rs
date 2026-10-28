//! Relational composition: inner and outer joins over one shared builder.

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
    // ANCHOR: inner
    let inner_join = select_from(User)
        .inner_join(Order, OrderUserId.eq(UserId))
        .select(cols!(UserId, UserName, OrderTotal))
        .build()
        .expect("valid inner join");
    // ANCHOR_END: inner
    println!("{}", inner_join.sql);

    // ANCHOR: outer
    let outer_join = select_from(User)
        .left_join(Order, OrderUserId.eq(UserId))
        .select(cols!(UserName, OrderTotal))
        .build()
        .expect("valid outer join");
    // ANCHOR_END: outer
    println!("{}", outer_join.sql);
}
