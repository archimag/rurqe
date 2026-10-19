//! End-to-end CRUD verification of the SQLite execution adapter.
//!
//! The test drives the complete mutation lifecycle through `rurqe::prelude`
//! against an in-memory SQLite database: a typed `INSERT ... RETURNING`, a
//! filtered `SELECT`, an `UPDATE ... RETURNING`, a `DELETE` reporting affected
//! rows, and an optional single-row lookup. Every statement is rendered by the
//! `Sqlite<Sqlite3_35>` dialect and decoded through the `QueryBackendExt`
//! helpers of `SqliteConnection`.
//!
//! SQLite stores booleans and 32-bit integers in the same `INTEGER` storage
//! class, so the fixture declares an `active` (`bool`) and a `score` (`i32`)
//! column: both are read back as `SqlVal::I64` and must still narrow into their
//! declared Rust types.

use rurqe::prelude::*;
use rurqe::sqlite::rusqlite::Connection;

table! {
    /// The `users` table.
    User => "users" {
        UserId => "id": i64,
        UserName => "name": String,
        UserActive => "active": bool,
        UserScore => "score": i32,
    }
}

/// DDL of the `users` fixture table.
const CREATE_USERS: &str = "CREATE TABLE users (
    id INTEGER NOT NULL PRIMARY KEY,
    name TEXT NOT NULL,
    active BOOLEAN NOT NULL,
    score INTEGER NOT NULL
);";

/// Opens an in-memory SQLite database holding an empty `users` table.
fn database() -> SqliteConnection {
    let connection = Connection::open_in_memory().expect("open in-memory database");
    connection
        .execute_batch(CREATE_USERS)
        .expect("create users table");
    SqliteConnection::new(connection)
}

#[test]
fn sqlite_crud_cycle_round_trips_through_the_facade() {
    let mut db = database();

    let insert_root = insert_into::<Sqlite<Sqlite3_35>, User>()
        .values((
            UserId::set(1i64),
            UserName::set("root"),
            UserActive::set(true),
            UserScore::set(42i32),
        ))
        .returning((UserId, UserName, UserActive, UserScore));
    let inserted = db.fetch_one(&insert_root).expect("insert returns its row");
    assert_eq!(inserted, (1i64, "root".to_owned(), true, 42i32));

    let insert_guest = insert_into::<Sqlite<Sqlite3_35>, User>().values((
        UserId::set(2i64),
        UserName::set("guest"),
        UserActive::set(false),
        UserScore::set(7i32),
    ));
    let affected = db
        .execute(&insert_guest)
        .expect("insert affects one row");
    assert_eq!(affected, 1);

    let active_query = select_from::<Sqlite<Sqlite3_35>, User>()
        .select((UserId, UserName, UserScore))
        .where_(Expr::column::<UserActive>().eq(true))
        .order_by_asc(UserId);
    let active = db
        .fetch_all(&active_query)
        .expect("select returns matching rows");
    assert_eq!(active, vec![(1i64, "root".to_owned(), 42i32)]);

    let rename = update::<Sqlite<Sqlite3_35>, User>()
        .set(UserName::set("bob"))
        .where_(Expr::column::<UserId>().eq(1i64))
        .returning((UserName, UserScore));
    let renamed = db.fetch_one(&rename).expect("update returns its row");
    assert_eq!(renamed, ("bob".to_owned(), 42i32));

    let delete_query =
        delete_from::<Sqlite<Sqlite3_35>, User>().where_(Expr::column::<UserId>().eq(1i64));
    let removed = db
        .execute(&delete_query)
        .expect("delete affects one row");
    assert_eq!(removed, 1);

    let missing_query = select_from::<Sqlite<Sqlite3_35>, User>()
        .select((UserId,))
        .where_(Expr::column::<UserId>().eq(1i64));
    let missing = db
        .fetch_optional(&missing_query)
        .expect("optional lookup succeeds");
    assert_eq!(missing, None);

    let remaining_query = select_from::<Sqlite<Sqlite3_35>, User>()
        .select((UserId, UserName, UserActive, UserScore))
        .order_by_asc(UserId);
    let remaining = db
        .fetch_all(&remaining_query)
        .expect("remaining rows decode");
    assert_eq!(remaining, vec![(2i64, "guest".to_owned(), false, 7i32)]);
}
