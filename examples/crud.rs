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
}
