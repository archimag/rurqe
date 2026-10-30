//! Relational composition: inner and outer joins over one shared builder.

use deadpool_postgres::Runtime;
use rurqe::postgres::prelude::*;
use tokio_postgres::NoTls;

table! {
    /// The `users` table.
    User => "users" {
        UserId => "id": i64,
        UserName => "name": String,
        UserManagerId => "manager_id": i64,    }
}

table! {
    /// The `orders` table.
    Order => "orders" {
        OrderId => "id": i64,
        OrderTotal => "total": f64,
        OrderUserId => "user_id": i64,
    }
}

table! {
    /// The `latest_order` lateral relation marker.
    LatestOrder => "latest_order" {
        LatestOrderTotal => "total": f64,
    }
}

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
    let outer_join = select_from(User)
        .left_join(Order, OrderUserId.eq(UserId))
        .select(cols!(UserName, OrderTotal))
        .fetch_all(&pool)
        .await?;
// ANCHOR_END: outer
    println!("{}", outer_join.len());

// ANCHOR: duplicate_refused
    // The same relation is addressed twice without an alias, so the `FROM` clause
    // and the join both name `users`. No engine accepts the statement, and the
    // builder refuses it before any SQL is sent to the database.
    let duplicate = select_from(User)
        .inner_join(User, UserManagerId.eq(UserId).without_scope())
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
        .where_(OrderUserId.eq(UserId.without_scope()))
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
