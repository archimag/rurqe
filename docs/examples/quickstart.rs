//! The shortest complete walkthrough: take the schema markers a project
//! generates from its catalog, build a typed query over them, and execute it
//! against an asynchronous pool.
//!
//! Examples compile without a database: constructing a pool opens no
//! connection, so `main` is compiled but never executed by the verification
//! pipeline.

use rurqe::postgres::prelude::*;
use rurqe_examples::example_pool;

// A project ships the module the generator writes as `src/schema.rs` and
// declares `mod schema;` in its crate root. The Quickstart chapter quotes that
// module from the fixture in `generated/`, because the generator renders it
// from a catalog rather than from a declaration somebody keeps in step by hand.
// An example target is a single file, so the declaration below is the `table!`
// marker the generated module carries for the same table, and the markers of
// both forms are addressed identically.
table!(
    /// The `users` table.
    Users => "users" {
        UsersEmail => "email": String,
        UsersId => "id": i64,
        UsersName => "name": String,
    }
);

// ANCHOR: dto
dto! {
    /// A user row crossing a function boundary.
    pub struct UserRow {
        UsersId as user_id,
        UsersName as contact_email,
    }
}
// ANCHOR_END: dto

#[tokio::main]
async fn main() -> Result<()> {
    let pool = example_pool();

    // ANCHOR: query
    // The statement names the schema markers, so the relation and its columns
    // are the ones the database actually has.
    let users = select_from(Users)
        .select(cols!(UsersId, UsersName, UsersEmail))
        .where_(UsersId.eq(7i64))
        .fetch_all(&pool)
        .await?;
    // ANCHOR_END: query
    println!("{}", users.len());

    // ANCHOR: named_rows
    // The projection decides the field names, so a fetched row is read by the
    // name its item declared instead of by a position in the row.
    let named = select_from(Users)
        .select(cols!(user_id: UsersId, contact_email: UsersName, user_email: UsersEmail))
        .where_(UsersEmail.eq("root@example.com"))
        .order_by_asc(UsersId)
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
    let filtered = select_from(Users)
        .select(cols!(UsersId, UsersName, UsersEmail))
        .when(only_active, |query| {
            query.where_(UsersEmail.ilike("%@example.com"))
        })
        .if_some(search, |query, term| {
            query.where_(UsersName.ilike(format!("%{term}%")))
        })
        .fetch_all(&pool)
        .await?;
    // ANCHOR_END: conditional
    println!("{}", filtered.len());

    // ANCHOR: build_refusal
    // Syntactically sound but semantically invalid: `DISTINCT ON` deduplicates
    // the rows on one expression while the leading `ORDER BY` criterion names
    // another, so no engine would accept the statement.
    let rejected = select_from(Users)
        .select(cols!(UsersId, UsersName))
        .distinct_on(UsersName)
        .order_by_asc(UsersId)
        .build();
    assert!(matches!(
        rejected,
        Err(BuildError::DistinctOnOrderMismatch { .. })
    ));
    // ANCHOR_END: build_refusal

    Ok(())
}

// ANCHOR: fallback
// Where no catalog is reachable — an isolated unit test, a prototype built
// without a database — the markers are declared by hand.
table!(
    /// The `audit_events` table.
    AuditEvents => "audit_events" {
        AuditEventsAction => "action": String,
        AuditEventsId => "id": i64,
    }
);
// ANCHOR_END: fallback

