//! Row mutations: `INSERT`, `UPDATE`, `DELETE`, `RETURNING`, and upsert.

use rurqe::postgres::prelude::*;

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
fn main() {
    // ANCHOR: insert
    let insert_query = insert_into(User)
        .values((
            UserId::set(1i64),
            UserName::set("root"),
            UserEmail::set("root@example.com"),
        ))
        .returning(cols!(UserId))
        .build()
        .expect("valid insert");
    // ANCHOR_END: insert
    println!("{}", insert_query.sql);

    // ANCHOR: update
    let update_query = update(User)
        .set(UserName::set("admin"))
        .where_(Expr::column::<UserId>().eq(1i64))
        .build()
        .expect("valid update");
    // ANCHOR_END: update
    println!("{}", update_query.sql);

    // ANCHOR: delete
    let delete_query = delete_from(User)
        .where_(Expr::column::<UserId>().eq(1i64))
        .build()
        .expect("valid delete");
    // ANCHOR_END: delete
    println!("{}", delete_query.sql);

    // ANCHOR: upsert
    let upsert_query = insert_into(User)
        .values((UserId::set(1i64), UserName::set("root")))
        .on_conflict_do_nothing()
        .build()
        .expect("valid upsert");
    // ANCHOR_END: upsert
    println!("{}", upsert_query.sql);

    // ANCHOR: values_many
    let bulk = insert_into(User)
        .values_many([
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
        ])
        .expect("the row source is not empty")
        .build()
        .expect("valid bulk insert");
    // ANCHOR_END: values_many
    println!("{}", bulk.sql);

    // ANCHOR: from_select
    // The source projection is a tuple of the inserted columns: `from_select`
    // ties the row type of the `SELECT` to the target column list.
    let source = select_from(ArchivedUser)
        .select((ArchivedUserId, ArchivedUserName, ArchivedUserEmail))
        .where_(Expr::column::<ArchivedUserId>().gt(0i64));
    let copied = insert_into(User)
        .from_select((UserId, UserName, UserEmail), source)
        .build()
        .expect("valid insert from select");
    // ANCHOR_END: from_select
    println!("{}", copied.sql);

    // ANCHOR: update_from
    let linked = update(User)
        .set(UserName::set("root"))
        .from_table(Account)
        .where_(Expr::column::<UserId>().eq(Expr::column::<AccountId>()))
        .build()
        .expect("valid update from");
    // ANCHOR_END: update_from
    println!("{}", linked.sql);

    // ANCHOR: delete_using
    let cleaned = delete_from(User)
        .using_table(Account)
        .where_(Expr::column::<UserId>().eq(Expr::column::<AccountId>()))
        .build()
        .expect("valid delete using");
    // ANCHOR_END: delete_using
    println!("{}", cleaned.sql);
}
