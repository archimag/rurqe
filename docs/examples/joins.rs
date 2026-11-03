//! Relational composition: inner and outer joins over one shared builder.

use rurqe::postgres::prelude::*;
use rurqe_examples::example_pool;
use rurqe_examples::markers::*;

table!(
    /// The `latest_order` lateral relation marker.
    LatestOrder => "latest_order" {
        LatestOrderTotal => "total": f64,
    }
);

#[tokio::main]
async fn main() -> Result<()> {
    let pool = example_pool();

    // ANCHOR: inner
    let inner_join = select_from(User)
        .inner_join(Order, OrderUserId.eq(UserId))
        .select(cols!(UserId, UserName, OrderTotal))
        .fetch_all(&pool)
        .await?;
    // ANCHOR_END: inner
    println!("{}", inner_join.len());

    // ANCHOR: outer
    // The entry point names the first relation, the join adds the second, and
    // the projection follows once the relations the statement reads are declared.
    let outer_join = select_from(User)
        .left_join(Order, OrderUserId.eq(UserId))
        .select(cols!(UserId, UserName, OrderTotal))
        .fetch_all(&pool)
        .await?;
    // ANCHOR_END: outer
    // ANCHOR: outer_rows
    // The fetched rows are read by field name: the projection decided the
    // names, and the left join decided the type of `order_total`.
    for row in &outer_join {
        println!("user #{}: {} spent {:?}", row.user_id, row.user_name, row.order_total);
    }
    // ANCHOR_END: outer_rows

    // ANCHOR: duplicate_refused
    // The same relation is addressed twice without an alias, so the `FROM` clause
    // and the join both name `users`. No engine accepts the statement, and the
    // builder refuses it before any SQL is sent to the database.
    let duplicate = select_from(User)
        .inner_join(User, outer(UserManagerId.eq(UserId)))
        .select((Expr::count_all(),))
        .build();
    assert!(matches!(
        duplicate,
        Err(BuildError::DuplicateRelation { name: "users" })
    ));
    // ANCHOR_END: duplicate_refused

    // ANCHOR: lateral
    // The correlated subquery is published as the relation marker `LatestOrder`,
    // so the enclosing statement reads its rows like those of a stored table.
    let latest_order = select_from(Order)
        .select((OrderTotal,))
        .where_(OrderUserId.eq(outer(UserId)))
        .order_by_desc(OrderId)
        .limit(1)
        .as_lateral::<LatestOrder>();
    let report = select_from(User)
        .left_join_lateral(latest_order)
        .select(cols!(UserId, LatestOrderTotal))
        .fetch_all(&pool)
        .await?;
    // ANCHOR_END: lateral
    println!("{}", report.len());
    Ok(())
}
