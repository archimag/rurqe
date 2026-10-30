//! Window functions and their specifications.

use rurqe::postgres::prelude::*;
use rurqe_examples::example_pool;
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

