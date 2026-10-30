//! Table aliases: unambiguous self-joins.

use deadpool_postgres::Runtime;
use rurqe::postgres::prelude::*;
use tokio_postgres::NoTls;

table! {
    /// The `users` table.
    User => "users" {
        UserId => "id": i64,
        UserName => "name": String,
        UserManagerId => "manager_id": i64,
    }
}

// ANCHOR: aliases
table_alias! {
    Manager => User as "m"
}

table_alias! {
    Employee => User as "e"
}
// ANCHOR_END: aliases

#[tokio::main]
async fn main() -> Result<()> {
    let pool = example_pool();

    // ANCHOR: self_join
    let managed = select_from(User.as_alias(Manager))
        .inner_join(
            User.as_alias(Employee),
            Employee.UserManagerId.eq(Manager.UserId),
        )
        .select(cols!(
            manager_name: Manager.UserName,
            employee_name: Employee.UserName,
        ))
        .fetch_all(&pool)
        .await?;
    // ANCHOR_END: self_join
    println!("{}", managed.len());

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
