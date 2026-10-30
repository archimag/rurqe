//! Row filters: the operator surface, runtime assembly, and subquery filters.

use deadpool_postgres::Runtime;
use rurqe::postgres::prelude::*;
use tokio_postgres::NoTls;

table! {
    /// The `users` table.
    User => "users" {
        UserId => "id": i64,
        UserName => "name": String,
        UserActive => "active": bool,
    }
}

table! {
    /// The `orders` table.
    Order => "orders" {
        OrderId => "id": i64,
        OrderUserId => "user_id": i64,
        OrderTotal => "total": f64,
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let pool = example_pool();

    // ANCHOR: operators
    let rows = select_from(User)
        .select(cols!(UserId, UserName))
        .where_(UserActive.eq(true))
        .where_(UserName.like("a%"))
        .order_by_asc(UserId)
        .fetch_all(&pool)
        .await?;
    // ANCHOR_END: operators
    println!("{}", rows.len());

    // ANCHOR: ranges
    let ranged = select_from(Order)
        .select(cols!(OrderId, OrderTotal))
        .where_(OrderTotal.between(10.0f64, 100.0f64))
        .where_(OrderUserId.in_list(vec![1i64, 2i64, 3i64]))
        .where_(OrderTotal.is_not_null())
        .fetch_all(&pool)
        .await?;
    // ANCHOR_END: ranges
    println!("{}", ranged.len());

    // ANCHOR: predicate_reuse
    let is_active = UserActive.eq(true);

    let active = select_from(User)
        .select(cols!(UserId, UserName))
        .where_(is_active)
        .fetch_all(&pool)
        .await?;
    // ANCHOR_END: predicate_reuse
    println!("{}", active.len());

    // ANCHOR: dynamic
    let only_active = true;
    let name: Option<&str> = None;

    let filtered = select_from(User)
        .select(cols!(user_id: UserId, user_name: UserName))
        .when(only_active, |query| query.where_(UserActive.eq(true)))
        .if_some(name, |query, name| query.where_(UserName.eq(name)))
        .fetch_all(&pool)
        .await?;
    // ANCHOR_END: dynamic
    println!("{}", filtered.len());

    // ANCHOR: ilike
    let admins = select_from(User)
        .select(cols!(UserName))
        .where_(UserName.ilike("a%"))
        .fetch_all(&pool)
        .await?;
    // ANCHOR_END: ilike
    println!("{}", admins.len());

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
