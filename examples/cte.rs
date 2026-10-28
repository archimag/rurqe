//! Common table expressions: aggregate a relation once, name the result, and
//! read it back as a relation of the same statement.

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
        OrderAmount => "amount": f64,
        OrderUserId => "user_id": i64,
    }
}

table! {
    /// The `user_totals` relation the aggregate is published under.
    UserTotals => "user_totals" {
        TotalsOwnerId => "owner_id": i64,
        TotalsTotal => "total": f64,
    }
}

fn main() {
    // ANCHOR: summary
    let totals = select_from(Order)
        .group_by(OrderUserId)
        .select(cols! {
            owner_id: OrderUserId,
            total: sum(OrderAmount),
        })
        .as_cte::<UserTotals>();
    // ANCHOR_END: summary

    // ANCHOR: report
    let report = select_from(User)
        .with(totals)
        .inner_join(UserTotals, TotalsOwnerId.eq(UserId))
        .select(cols! {
            user_name: UserName,
            total_spent: TotalsTotal,
        })
        .build()
        .expect("valid CTE query");
    // ANCHOR_END: report

    println!("{}", report.sql);
}
