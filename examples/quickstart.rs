//! The shortest complete walkthrough: declare a table, build a typed query,
//! and read the SQL the builder produced.
//!
//! Examples compile without a database: building a query never opens a
//! connection, so `main` prints the SQL instead of executing it.

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
    // ANCHOR: query
    let built = select_from(User)
        .select(cols!(UserId, UserName, UserEmail))
        .where_(Expr::column::<UserId>().eq(7i64))
        .build()
        .expect("valid query");
    // ANCHOR_END: query

    println!("{}", built.sql);
}
