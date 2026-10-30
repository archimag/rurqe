//! Row mutations: `INSERT`, `UPDATE`, `DELETE`, `RETURNING`, and upsert.
//!
//! Every mutation except the bulk insert terminates against an asynchronous
//! pool, so a chapter includes the shape an application actually writes; the
//! bulk insert is assembled as a build fragment of its own.

use deadpool_postgres::Runtime;
use rurqe::postgres::prelude::*;
use tokio_postgres::NoTls;

table! {
    /// The `users` table.
    User => "users" {
        UserId => "id": i64,
        UserName => "name": String,
        UserEmail => "email": String,
    }
}

table! {
    /// The `archived_users` table.
    ArchivedUser => "archived_users" {
        ArchivedUserId => "id": i64,
        ArchivedUserName => "name": String,
        ArchivedUserEmail => "email": String,
    }
}

table! {
    /// The `accounts` table.
    Account => "accounts" {
        AccountId => "id": i64,
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let pool = example_pool();

    // ANCHOR: insert
    let inserted = insert_into(User)
        .values((
            UserId::set(1i64),
            UserName::set("root"),
            UserEmail::set("root@example.com"),
        ))
        .returning(cols!(UserId))
        .fetch_one(&pool)
        .await?;
    // ANCHOR_END: insert
    println!("inserted {inserted:?}");

    // ANCHOR: update
    let updated = update(User)
        .set(UserName::set("admin"))
        .where_(UserId.eq(1i64))
        .execute(&pool)
        .await?;
    // ANCHOR_END: update
    println!("updated {updated} row(s)");

    // ANCHOR: delete
    let removed = delete_from(User)
        .where_(UserId.eq(1i64))
        .execute(&pool)
        .await?;
    // ANCHOR_END: delete
    println!("removed {removed} row(s)");

    // ANCHOR: upsert
    let upserted = insert_into(User)
        .values((UserId::set(1i64), UserName::set("root")))
        .on_conflict_do_nothing()
        .execute(&pool)
        .await?;
    // ANCHOR_END: upsert
    println!("upserted {upserted} row(s)");

    let _ = assemble_bulk_insert;

    // ANCHOR: from_select
    // The source projection is a tuple of the inserted columns: `from_select`
    // ties the row type of the `SELECT` to the target column list.
    let source = select_from(ArchivedUser)
        .select((ArchivedUserId, ArchivedUserName, ArchivedUserEmail))
        .where_(ArchivedUserId.gt(0i64));
    let copied = insert_into(User)
        .from_select((UserId, UserName, UserEmail), source)
        .execute(&pool)
        .await?;
    // ANCHOR_END: from_select
    println!("copied {copied} row(s)");

    // ANCHOR: update_from
    let linked = update(User)
        .set(UserName::set("root"))
        .from_table(Account)
        .where_(UserId.eq(AccountId))
        .execute(&pool)
        .await?;
    // ANCHOR_END: update_from
    println!("linked {linked} row(s)");

    // ANCHOR: update_returning
    let renamed = update(User)
        .set(UserName::set("root"))
        .where_(UserId.eq(1i64))
        .returning(cols!(UserName))
        .fetch_all(&pool)
        .await?;
    // ANCHOR_END: update_returning
    println!("renamed {renamed:?}");

    // ANCHOR: delete_using
    let cleaned = delete_from(User)
        .using_table(Account)
        .where_(UserId.eq(AccountId))
        .execute(&pool)
        .await?;
    // ANCHOR_END: delete_using
    println!("cleaned {cleaned} row(s)");

    // ANCHOR: delete_returning
    let removed = delete_from(User)
        .where_(UserId.eq(1i64))
        .returning(cols!(UserName))
        .fetch_all(&pool)
        .await?;
    // ANCHOR_END: delete_returning
    println!("removed rows: {removed:?}");

    Ok(())
}

/// Assembles the bulk insert as a build fragment of its own.
///
/// The row source is the fallible step: an empty list is refused instead of
/// rendering a `VALUES` list with no row in it, so the fragment carries
/// `EmptyRows` rather than the driver error of the pool.
fn assemble_bulk_insert() -> Result<(), EmptyRows> {
    // ANCHOR: values_many
    insert_into(User).values_many([
        (
            UserId::set(1i64),
            UserName::set("alice"),
            UserEmail::set("alice@example.com"),
        ),
        (
            UserId::set(2i64),
            UserName::set("bob"),
            UserEmail::set("bob@example.com"),
        ),
    ])?;
    // ANCHOR_END: values_many
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
