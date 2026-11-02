//! Typed nested collections from a single round trip: `cols!` with
//! `nest_many` and `nest_one`, so the children of every parent arrive as one
//! JSON document.

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
        OrderTotal => "total": f64,
        OrderUserId => "user_id": i64,
    }
);

// ANCHOR: document_row
dto! {
    /// One child row decoded from the aggregated document of its parent.
    pub struct OrderLine {
        OrderId as order_id,
        OrderTotal as total,
    }
}
// ANCHOR_END: document_row

#[tokio::main]
async fn main() -> Result<()> {
    let pool = example_pool();
    // ANCHOR: nested
    let users_with_orders = select_from(User)
        .select(cols! {
            user_name: UserName,
            orders: nest_many(
                select_from(Order)
                    .select(cols! {
                        order_id: OrderId,
                        total: OrderTotal,
                    })
                    .where_(OrderUserId.eq(outer(UserId)))
                    .order_by_desc(OrderTotal),
            ),
        })
        .fetch_all(&pool)
        .await?;
    // ANCHOR_END: nested
    println!("{}", users_with_orders.len());

    // ANCHOR: nest_one
    // The same shape for a one-to-one relation: the collection stops at the
    // first row, so the field reads as an `Option` of the nested projection
    // and an absent child stays distinguishable from a child without values.
    let users_with_top_order = select_from(User)
        .select(cols! {
            user_name: UserName,
            top_order: nest_one(
                select_from(Order)
                    .select(cols! {
                        order_id: OrderId,
                        total: OrderTotal,
                    })
                    .where_(OrderUserId.eq(outer(UserId)))
                    .order_by_desc(OrderTotal),
            ),
        })
        .fetch_all(&pool)
        .await?;
    // ANCHOR_END: nest_one
    println!("{}", users_with_top_order.len());

    // ANCHOR: collection
    let lines = select_from(Order).select(cols!(order_id: OrderId, total: OrderTotal));
    let parents = select_from(User)
        .select((UserId, nest_many(lines)))
        .fetch_all(&pool)
        .await?;
    // ANCHOR_END: collection
    println!("{}", parents.len());

    // ANCHOR: named_collection
    let named_lines = select_from(Order).select(cols!(order_id: OrderId, total: OrderTotal));
    let named_parents = select_from(User)
        .select(cols! {
            user_id: UserId,
            orders: nest_many(named_lines),
        })
        .fetch_all(&pool)
        .await?;
    // ANCHOR_END: named_collection
    for parent in &named_parents {
        println!("#{} {}", parent.user_id, parent.orders.len());
    }

    // ANCHOR: single
    let single_line = select_from(Order).select(cols!(total: OrderTotal));
    let parents_with_one = select_from(User)
        .select((UserId, nest_one(single_line)))
        .fetch_all(&pool)
        .await?;
    // ANCHOR_END: single
    println!("{}", parents_with_one.len());

    let _ = (build_correlated, build_sqlite_collection, decode_document);

    Ok(())
}

/// Builds the correlated statement as a fragment of its own.
fn build_correlated() -> Result<(), BuildError> {
    // ANCHOR: correlation
    // The child statement introduces only `orders` in its scope, so a column
    // of the enclosing query is marked as resolved elsewhere instead of being
    // looked up among the relations the child statement declares.
    let _correlated = select_from(Order)
        .select((OrderUserId,))
        .where_(OrderUserId.eq(outer(UserId)))
        .build()?;
    // ANCHOR_END: correlation
    Ok(())
}

/// Assembles the same collection for a SQLite target.
///
/// SQLite has no native row aggregation, so the collection is assembled from
/// `json_object` fields collected by `json_group_array`. The statement is only
/// assembled here, so the fragment names no runtime of its own.
fn build_sqlite_collection() -> Result<(), BuildError> {
    // ANCHOR: collection_sqlite
    let lines = rurqe::sqlite::sqlite3_38::select_from(Order)
        .select(cols!(order_id: OrderId, total: OrderTotal));
    let statement = rurqe::sqlite::sqlite3_38::select_from(User)
        .select((UserId, nest_many(lines)))
        .build()?;
    // ANCHOR_END: collection_sqlite
    println!("{}", statement.sql);
    Ok(())
}

/// Decodes the aggregated document of a nested collection.
///
/// PostgreSQL reports the document of a collection as a JSON value and SQLite
/// reports it as text; `from_sql_value` accepts both forms.
fn decode_document() -> Result<(), RowDecodeError> {
    // ANCHOR: document_decode
    // The keys of the document are the field names of the nested projection, so
    // the declared row reads them without a positional index.
    let document = r#"[{"order_id":1,"total":19.5},{"order_id":2,"total":7.25}]"#;
    let lines = Vec::<OrderLine>::from_sql_value(&SqlVal::Text(document.to_owned()))?;
    // ANCHOR_END: document_decode
    for line in &lines {
        println!("#{} {}", line.order_id, line.total);
    }
    Ok(())
}

