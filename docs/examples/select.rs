//! The read layer: projections, field naming, and `DISTINCT`.

use rurqe::postgres::prelude::*;
use rurqe_examples::example_pool;
table!(
    /// The `users` table.
    User => "users" {
        UserId => "id": i64,
        UserName => "name": String,
        UserEmail => "email": String,
        UserActive => "active": bool,
    }
);

table!(
    /// The `orders` table.
    Order => "orders" {
        OrderId => "id": i64,
        OrderTotal => "total": f64,
        OrderTax => "tax": f64,
    }
);

// ANCHOR: dto_declaration
dto! {
    /// A user row crossing a function boundary.
    pub struct UserSummary {
        UserId,
        UserName as contact_email,
    }
}
// ANCHOR_END: dto_declaration

#[tokio::main]
async fn main() -> Result<()> {
    let pool = example_pool();

    // ANCHOR: select_head
    // `select!` heads the chain: the projection comes first, and the relation
    // is named right after it.
    let named = select!(UserId, UserName as contact_email)
        .from(User)
        .where_(UserActive.eq(true))
        .fetch_all(&pool)
        .await?;
    // ANCHOR_END: select_head
    for row in &named {
        println!("#{} <{}>", row.user_id, row.contact_email);
    }

    // ANCHOR: aliased_fields
    let active = select_from(User)
        .select(cols!(user_id: UserId, contact_email: UserEmail))
        .where_(UserActive.eq(true))
        .order_by_asc(UserId)
        .fetch_all(&pool)
        .await?;
    // ANCHOR_END: aliased_fields
    for row in active {
        println!("#{} <{}>", row.user_id, row.contact_email);
    }

    // ANCHOR: computed
    let billed = select_from(Order)
        .select(cols!(
            order_id: OrderId,
            total_with_tax: OrderTotal + OrderTax,
        ))
        .fetch_all(&pool)
        .await?;
    // ANCHOR_END: computed
    println!("{}", billed.len());

    // ANCHOR: distinct
    let unique_names = select_from(User)
        .select(cols!(UserName))
        .distinct()
        .order_by_asc(UserName)
        .fetch_all(&pool)
        .await?;
    // ANCHOR_END: distinct
    println!("{}", unique_names.len());

    // ANCHOR: dto_decode
    // The row is read through the declaration: the fields keep the types the
    // schema declares, and the column order of the statement does not matter.
    let summary = UserSummary::from_row(&[SqlVal::I64(7), SqlVal::Text("root".to_owned())]);
    // ANCHOR_END: dto_decode
    println!("{summary:?}");

    Ok(())
}
