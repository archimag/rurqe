//! Window functions and their specifications.

use deadpool_postgres::Runtime;
use rurqe::postgres::prelude::*;
use tokio_postgres::NoTls;

table! {
    /// The `employees` table.
    Employee => "employees" {
        EmployeeId => "id": i64,
        DepartmentId => "department_id": i64,
        Salary => "salary": i64,
    }
}

table! {
    /// The `orders` table.
    Order => "orders" {
        OrderId => "id": i64,
        OrderCreatedAt => "created_at": i64,
        OrderTotal => "total": f64,
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let pool = example_pool();

    // ANCHOR: ranked
    let ranked = select_from(Employee)
        .select(cols!(
            employee_id: EmployeeId,
            position: row_number().over(
                Window::new()
                    .partition_by(DepartmentId)
                    .order_by_desc(Salary),
            ),
        ))
        .fetch_all(&pool)
        .await?;
    // ANCHOR_END: ranked
    for row in ranked {
        println!("{} at {}", row.employee_id, row.position);
    }

    // ANCHOR: frames
    let moving = select_from(Order)
        .select(cols!(
            order_id: OrderId,
            running_total: sum(OrderTotal).over(
                Window::order_by_asc(OrderCreatedAt).rows_between(
                    WindowFrameBound::preceding(2),
                    WindowFrameBound::CurrentRow,
                ),
            ),
        ))
        .fetch_all(&pool)
        .await?;
    // ANCHOR_END: frames
    println!("{}", moving.len());

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
