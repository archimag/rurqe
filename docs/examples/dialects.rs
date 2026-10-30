//! One query, three dialect modules: identifier quoting, placeholders, and
//! the pinned capability set differ, the typed builder stays the same. The file
//! only assembles and renders statements, so it names no runtime at all.

use rurqe::postgres::prelude::*;
use rurqe::sqlite::{sqlite3_0, sqlite3_35};

table! {
    /// The `users` table.
    User => "users" {
        UserId => "id": i64,
    }
}

fn main() -> Result<(), BuildError> {
// ANCHOR: postgres
    let postgres_query = select_from(User)
        .select(cols!(UserId))
        .where_(UserId.eq(7i64))
        .build()?;
// ANCHOR_END: postgres
    println!("{}", postgres_query.sql);

// ANCHOR: sqlite
    let sqlite_query = sqlite3_35::select_from(User)
        .select(cols!(UserId))
        .where_(UserId.eq(7i64))
        .build()?;
// ANCHOR_END: sqlite
    println!("{}", sqlite_query.sql);

// ANCHOR: version
    // The version marker pins the capability set, not the builder: the same
    // statement composes for any pinned release, and only the clauses that
    // release does not support stay unreachable.
    let legacy_query = sqlite3_0::select_from(User)
        .select(cols!(UserId))
        .where_(UserId.eq(7i64))
        .build()?;
// ANCHOR_END: version
    println!("{}", legacy_query.sql);
    Ok(())
}
