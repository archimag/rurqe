//! The shortest complete walkthrough: declare a table, build a typed query,
//! and execute it against an asynchronous pool.
//!
//! Examples compile without a database: constructing a pool opens no
//! connection, so `main` is compiled but never executed by the verification
//! pipeline.

use rurqe::postgres::prelude::*;
use rurqe_examples::example_pool;
// ANCHOR: schema
table! {
    /// The `users` table.
    User => "users" {
        UserId => "id": i64,
        UserName => "name": String,
        UserEmail => "email": String,
    }
}
// ANCHOR_END: schema

// ANCHOR: dto
dto! {
    /// A user row crossing a function boundary.
    pub struct UserRow {
        UserId,
        UserName as contact_email,
    }
}
// ANCHOR_END: dto

#[tokio::main]
async fn main() -> Result<()> {
    let pool = example_pool();

    // ANCHOR: query
    let users = select_from(User)
        .select(cols!(UserId, UserName, UserEmail))
        .where_(UserId.eq(7i64))
        .fetch_all(&pool)
        .await?;
    // ANCHOR_END: query
    println!("{}", users.len());

    // ANCHOR: named_rows
    // The projection decides the field names, so a fetched row is read by the
    // name its item declared instead of by a position in the row.
    let named = select_from(User)
        .select(cols!(UserId, UserName as contact_email, UserEmail))
        .where_(UserEmail.eq("root@example.com"))
        .order_by_asc(UserId)
        .fetch_all(&pool)
        .await?;
    for row in &named {
        println!("#{} <{}> / {}", row.user_id, row.contact_email, row.user_email);
    }
    // ANCHOR_END: named_rows

    // ANCHOR: dto_row
    // The declared row carries the names of its declaration, so a decoded row is
    // read by name wherever the row is used.
    if let Some(row) = UserRow::from_row(&[SqlVal::I64(7), SqlVal::Text("root".to_owned())]) {
        println!("#{} <{}>", row.user_id, row.contact_email);
    }
    // ANCHOR_END: dto_row

    // ANCHOR: conditional
    // The flags a handler receives at runtime; a false flag or an absent value
    // adds no clause, and the statement keeps its type.
    let only_active = true;
    let search: Option<&str> = None;
    let filtered = select_from(User)
        .select(cols!(UserId, UserName, UserEmail))
        .when(only_active, |query| {
            query.where_(UserEmail.ilike("%@example.com"))
        })
        .if_some(search, |query, term| {
            query.where_(UserName.ilike(format!("%{term}%")))
        })
        .fetch_all(&pool)
        .await?;
    // ANCHOR_END: conditional
    println!("{}", filtered.len());

    // ANCHOR: build_refusal
    // Syntactically sound but semantically invalid: `DISTINCT ON` deduplicates
    // the rows on one expression while the leading `ORDER BY` criterion names
    // another, so no engine would accept the statement.
    let rejected = select_from(User)
        .select(cols!(UserId, UserName))
        .distinct_on(UserName)
        .order_by_asc(UserId)
        .build();
    assert!(matches!(
        rejected,
        Err(BuildError::DistinctOnOrderMismatch { .. })
    ));
    // ANCHOR_END: build_refusal

    Ok(())
}

