//! Row mutations: `INSERT`, `UPDATE`, `DELETE`, `RETURNING`, and upsert.
//!
//! Every mutation except the bulk insert terminates against an asynchronous
//! pool, so a chapter includes the shape an application actually writes; the
//! bulk insert is assembled as a build fragment of its own.
//!
//! The markers the fragments read come from `rurqe_examples::markers`, the
//! module every example shares, so a fragment the walkthrough parity test
//! rebuilds resolves the same declaration the chapter documents.

use rurqe::postgres::prelude::*;
use rurqe_examples::example_pool;
use rurqe_examples::markers::*;

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

    // ANCHOR: insert_nullable
    let stored = insert_into(Setting)
        .values((SettingId::set(2i64), SettingLocale::set(None::<String>)))
        .execute(&pool)
        .await?;
    // ANCHOR_END: insert_nullable
    println!("stored {stored} row(s)");

    // ANCHOR: update
    let updated = update(User)
        .set(UserName::set("admin"))
        .where_(UserId.eq(1i64))
        .execute(&pool)
        .await?;
    // ANCHOR_END: update
    println!("updated {updated} row(s)");

    // ANCHOR: update_nullable
    let cleared = update(Setting)
        .set(SettingLocale::set(None::<String>))
        .where_(SettingId.eq(1i64))
        .execute(&pool)
        .await?;
    // ANCHOR_END: update_nullable
    println!("cleared {cleared} row(s)");

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

    // The bulk insert is assembled rather than executed: its row source is
    // refused by the builder instead of the engine, so the fragment needs no
    // database. A literal list holds rows, so the refusal cannot fire, and the
    // demonstration stops instead of continuing past a statement it failed to
    // assemble.
    if let Err(refusal) = assemble_bulk_insert() {
        panic!("the documented bulk insert assembles its statement: {refusal}");
    }

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

    // ANCHOR: update_where_opt
    let target: Option<i64> = Some(1i64);
    let narrowed = update(User)
        .set(UserName::set("admin"))
        .where_opt(target.map(|value| UserId.eq(value)))
        .allow_all()
        .execute(&pool)
        .await?;
    // ANCHOR_END: update_where_opt
    println!("narrowed {narrowed} row(s)");

    // ANCHOR: update_where_if
    let target: Option<i64> = None;
    let confirmed = update(User)
        .set(UserName::set("admin"))
        .where_if(target.map(|value| UserId.eq(value)))
        .allow_all()
        .execute(&pool)
        .await?;
    // ANCHOR_END: update_where_if
    println!("confirmed {confirmed} row(s)");

    // ANCHOR: update_where_opt_or_all
    let target: Option<i64> = None;
    let widened = update(User)
        .set(UserName::set("admin"))
        .where_opt_or_all(target.map(|value| UserId.eq(value)))
        .execute(&pool)
        .await?;
    // ANCHOR_END: update_where_opt_or_all
    println!("widened {widened} row(s)");
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

    // ANCHOR: delete_where_opt
    let target: Option<i64> = Some(1i64);
    let matched = delete_from(User)
        .where_opt(target.map(|value| UserId.eq(value)))
        .allow_all()
        .execute(&pool)
        .await?;
    // ANCHOR_END: delete_where_opt
    println!("matched {matched} row(s)");

    // ANCHOR: delete_where_if
    let target: Option<i64> = None;
    let swept = delete_from(User)
        .where_if(target.map(|value| UserId.eq(value)))
        .allow_all()
        .execute(&pool)
        .await?;
    // ANCHOR_END: delete_where_if
    println!("swept {swept} row(s)");

    // ANCHOR: delete_where_opt_or_all
    let target: Option<i64> = None;
    let emptied = delete_from(User)
        .where_opt_or_all(target.map(|value| UserId.eq(value)))
        .execute(&pool)
        .await?;
    // ANCHOR_END: delete_where_opt_or_all
    println!("emptied {emptied} row(s)");

    // ANCHOR: server_refusal
    // A refusal the server described is read through the typed chain rather
    // than through the message it renders: the terminal call reports the
    // adapter failure as a backend error, and the adapter opens the description
    // the server sent — its SQLSTATE, its constraint, and its table.
    let refused = insert_into(User)
        .values((
            UserId::set(1i64),
            UserName::set("root"),
            UserEmail::set("root@example.com"),
        ))
        .execute(&pool)
        .await;

    match &refused {
        Err(ExecutionError::Backend(error)) => match error.as_db_error() {
            Some(db_error) if db_error.code() == &SqlState::UNIQUE_VIOLATION => println!(
                "refused by constraint {} on table {}",
                db_error.constraint().unwrap_or("(unnamed)"),
                db_error.table().unwrap_or("(unnamed)"),
            ),
            Some(db_error) => println!("the server refused the row: {}", db_error.message()),
            None => println!("the driver reported no server description: {error}"),
        },
        Err(other) => println!("the statement failed before the server described it: {other}"),
        Ok(rows) => println!("wrote {rows} row(s)"),
    }
    // ANCHOR_END: server_refusal

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
