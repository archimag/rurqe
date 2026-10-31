//! Common table expressions: aggregate a relation once, name the result, and
//! read it back as a relation of the same statement.

use rurqe::postgres::prelude::*;
use rurqe_examples::example_pool;
table!(
    /// The `users` table.
    User => "users" {
        UserId => "id": i64,
        UserName => "name": String,
    }
);

table!(
    /// The `orders` table.
    Order => "orders" {
        OrderId => "id": i64,
        OrderAmount => "amount": f64,
        OrderUserId => "user_id": i64,
    }
);

table!(
    /// The `user_totals` relation the aggregate is published under.
    UserTotals => "user_totals" {
        TotalsOwnerId => "owner_id": i64,
        TotalsTotal => "total": f64,
    }
);

#[tokio::main]
async fn main() -> Result<()> {
    let pool = example_pool();

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
        .fetch_all(&pool)
        .await?;
    // ANCHOR_END: report
    println!("{}", report.len());

    // ANCHOR: derived
    // The same aggregate published as a derived table: the statement keeps no
    // name of its own, so the join renders it inline as the relation it reads.
    let inline_totals = select_from(Order)
        .group_by(OrderUserId)
        .select(cols! {
            owner_id: OrderUserId,
            total: sum(OrderAmount),
        })
        .as_table::<UserTotals>();
    let inline_report = select_from(User)
        .inner_join_subquery(inline_totals, TotalsOwnerId.eq(UserId))
        .select(cols! {
            user_name: UserName,
            total_spent: TotalsTotal,
        })
        .fetch_all(&pool)
        .await?;
    // ANCHOR_END: derived
    println!("{}", inline_report.len());

    // ANCHOR: derived_left
    // A left-joined derived table lifts its columns: the aggregate may have no
    // row for a user, so `total_spent` arrives as `Option<f64>`.
    let optional_totals = select_from(Order)
        .group_by(OrderUserId)
        .select(cols! {
            owner_id: OrderUserId,
            total: sum(OrderAmount),
        })
        .as_table::<UserTotals>();
    let optional_report = select_from(User)
        .left_join_subquery(optional_totals, TotalsOwnerId.eq(UserId))
        .select(cols! {
            user_name: UserName,
            total_spent: TotalsTotal,
        })
        .fetch_all(&pool)
        .await?;
    // ANCHOR_END: derived_left
    println!("{}", optional_report.len());
    Ok(())
}

