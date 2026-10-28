//! One query, two dialects: identifier quoting and placeholders differ,
//! the typed builder stays the same.

use rurqe::postgres::prelude::*;
use rurqe::sqlite::sqlite3_35;

table! {
    /// The `users` table.
    User => "users" {
        UserId => "id": i64,
    }
}

fn main() {
    // ANCHOR: postgres
    let postgres_query = select_from(User)
        .select(cols!(UserId))
        .where_(Expr::column::<UserId>().eq(7i64))
        .build()
        .expect("valid postgres query");
    // ANCHOR_END: postgres
    println!("{}", postgres_query.sql);

    // ANCHOR: sqlite
    let sqlite_query = sqlite3_35::select_from(User)
        .select(cols!(UserId))
        .where_(Expr::column::<UserId>().eq(7i64))
        .build()
        .expect("valid sqlite query");
    // ANCHOR_END: sqlite
    println!("{}", sqlite_query.sql);
}
