//! Reusable query patterns: `macro_rules!` factoring of repeated query shapes
//! on the application side, and a named `dto!` row with a conversion into a
//! domain type.

use rurqe::postgres::prelude::*;
use rurqe_examples::example_pool;
use rurqe_examples::markers::*;

/// The domain type the application works with.
struct Customer {
    id: i64,
    name: String,
    email: String,
}

// ANCHOR: repository_macro
macro_rules! user_repository {
    (
        $domain:ty,
        find $find:ident,
        list $list:ident,
        select { $($projection:tt)* },
        convert $convert:expr
    ) => {
        async fn $find(pool: &PgAsyncPool, key: i64) -> Result<Option<$domain>> {
            let row = select_from(User)
                .select(cols!($($projection)*))
                .where_(UserId.eq(key))
                .fetch_optional(pool)
                .await?;
            Ok(row.map($convert))
        }

        async fn $list(
            pool: &PgAsyncPool,
            managed_only: bool,
            name: Option<&str>,
        ) -> Result<Vec<$domain>> {
            let rows = select_from(User)
                .select(cols!($($projection)*))
                .when(managed_only, |query| query.where_(UserManagerId.gt(0i64)))
                .if_some(name, |query, name| query.where_(UserName.eq(name)))
                .order_by_asc(UserId)
                .fetch_all(pool)
                .await?;
            Ok(rows.into_iter().map($convert).collect())
        }
    };
}
// ANCHOR_END: repository_macro

// ANCHOR: repository_instance
user_repository! {
    Customer,
    find find_customer,
    list list_customers,
    select { UserId, UserName, UserEmail },
    convert |row| Customer {
        id: row.user_id,
        name: row.user_name,
        email: row.user_email,
    }
}
// ANCHOR_END: repository_instance

// ANCHOR: dto_row
dto! {
    /// A user row that crosses a module boundary.
    pub struct UserRow {
        UserId,
        UserName,
        UserEmail,
    }
}
// ANCHOR_END: dto_row

// ANCHOR: dto_conversion
impl From<UserRow> for Customer {
    fn from(row: UserRow) -> Self {
        Customer {
            id: row.user_id,
            name: row.user_name,
            email: row.user_email,
        }
    }
}
// ANCHOR_END: dto_conversion

/// Converts the aggregated document of a nested collection into domain values.
fn customers_from_document(document: &str) -> Result<Vec<Customer>, RowDecodeError> {
    // ANCHOR: dto_document
    let rows = Vec::<UserRow>::from_sql_value(&SqlVal::Text(document.to_owned()))?;
    let customers: Vec<Customer> = rows.into_iter().map(Customer::from).collect();
    // ANCHOR_END: dto_document
    Ok(customers)
}

#[tokio::main]
async fn main() -> Result<()> {
    let pool = example_pool();

    if let Some(customer) = find_customer(&pool, 1).await? {
        println!("{} {} {}", customer.id, customer.name, customer.email);
    }

    let customers = list_customers(&pool, true, Some("alice")).await?;
    for customer in &customers {
        println!("{} {} {}", customer.id, customer.name, customer.email);
    }

    let document = r#"[{"user_id":1,"user_name":"root","user_email":"root@example.com"}]"#;
    let documented =
        customers_from_document(document).map_err(|source| ExecutionError::RowDecode {
            row_index: 0,
            source,
        })?;
    for customer in &documented {
        println!("{} {} {}", customer.id, customer.name, customer.email);
    }

    Ok(())
}
