//! Table aliases: unambiguous self-joins.

use rurqe::postgres::prelude::*;
use rurqe_examples::example_pool;
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

