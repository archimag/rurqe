//! Synchronous execution: the same statement, a blocking terminal call.
//!
//! The example drives an in-memory SQLite database, so the terminal call
//! returns a `Result` directly instead of a future the caller awaits. The
//! statement itself is built by the very same chain an asynchronous example
//! writes; only the terminal call differs.

use rurqe::sqlite::prelude::*;

table!(
    /// The `users` table.
    User => "users" {
        UserId => "id": i64,
    }
);

// ANCHOR: load_user
/// Reads the identifier of one user through a blocking terminal call.
fn load_user(conn: &mut SqliteConnection) -> Result<Option<i64>, Error> {
    let user = select_from(User)
        .select(cols!(UserId))
        .fetch_optional(conn)?;
    Ok(user.map(|row| row.user_id))
}
// ANCHOR_END: load_user

// ANCHOR: fetch_iter
/// Lazily streams matching rows one at a time.
fn stream_users(conn: &mut SqliteConnection) -> Result<i64, Error> {
    let query = select_from(User).select(cols!(UserId));
    let mut count = 0;
    for row in conn.fetch_iter(&query)? {
        let _id = row?.user_id;
        count += 1;
    }
    Ok(count)
}
// ANCHOR_END: fetch_iter

// ANCHOR: fetch_collect
/// Collects query rows directly into any collection implementing `FromIterator`.
fn collect_user_deque(conn: &mut SqliteConnection) -> Result<usize, Error> {
    let query = select_from(User).select(cols!(UserId));
    let rows: std::collections::VecDeque<_> = conn.fetch_collect(&query)?;
    Ok(rows.len())
}
// ANCHOR_END: fetch_collect
fn main() -> Result<()> {
    // ANCHOR: open_in_memory
    let mut conn = SqliteConnection::open_in_memory()?;
    // ANCHOR_END: open_in_memory
    conn.execute_batch("CREATE TABLE users (id INTEGER NOT NULL PRIMARY KEY);")?;

    match load_user(&mut conn)? {
        Some(id) => println!("user {id}"),
        None => println!("no user"),
    }

    let _ = stream_users(&mut conn)?;
    let _ = collect_user_deque(&mut conn)?;

    Ok(())
}
