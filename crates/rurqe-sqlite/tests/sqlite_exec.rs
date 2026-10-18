//! End-to-end execution tests for the `rusqlite` adapter against in-memory SQLite.

use rurqe_sqlite::rurqe_core::dml::{ColumnAssign, InsertQuery};
use rurqe_sqlite::rurqe_core::expr::Expr;
use rurqe_sqlite::rurqe_core::query::{QueryBackendExt, SelectQuery};
use rurqe_sqlite::rurqe_core::table;
use rurqe_sqlite::rusqlite::Connection;
use rurqe_sqlite::{Sqlite, Sqlite3_35, SqliteConnection};

table! {
    /// The `users` table.
    User => "users" {
        UserId => "id": i64,
        UserName => "name": String,
    }
}

/// DDL for the `users` fixture table.
const CREATE_USERS: &str =
    "CREATE TABLE users (id INTEGER NOT NULL PRIMARY KEY, name TEXT NOT NULL);";

/// Creates a fixture connection with the `users` table and two seeded rows.
fn seeded() -> SqliteConnection {
    let connection = Connection::open_in_memory().expect("open in-memory database");
    connection
        .execute_batch(CREATE_USERS)
        .expect("create users table");
    connection
        .execute("INSERT INTO users (id, name) VALUES (?1, ?2)", (1i64, "root"))
        .expect("insert first row");
    connection
        .execute("INSERT INTO users (id, name) VALUES (?1, ?2)", (2i64, "guest"))
        .expect("insert second row");
    SqliteConnection::new(connection)
}

#[test]
fn execute_reports_affected_rows_from_insert() {
    let connection = Connection::open_in_memory().expect("open in-memory database");
    connection
        .execute_batch(CREATE_USERS)
        .expect("create users table");
    let mut adapter = SqliteConnection::new(connection);

    let insert = InsertQuery::<Sqlite<Sqlite3_35>, User>::new()
        .values((UserId::set(1i64), UserName::set("root")));

    let affected = adapter.execute(&insert).expect("insert succeeds");
    assert_eq!(affected, 1);
}

#[test]
fn fetch_all_decodes_ordered_rows_with_parameters() {
    let mut adapter = seeded();

    let query = SelectQuery::<Sqlite<Sqlite3_35>>::new()
        .from::<User>()
        .select((UserId, UserName))
        .where_(Expr::column::<UserId>().gt(0i64))
        .order_by_asc(UserId);

    let rows = adapter.fetch_all(&query).expect("fetch all succeeds");

    assert_eq!(
        rows,
        vec![(1i64, "root".to_owned()), (2i64, "guest".to_owned())]
    );
}

#[test]
fn fetch_one_decodes_a_single_row() {
    let mut adapter = seeded();

    let query = SelectQuery::<Sqlite<Sqlite3_35>>::new()
        .from::<User>()
        .select((UserName,))
        .where_(Expr::column::<UserId>().eq(2i64));

    let name = adapter.fetch_one(&query).expect("one row");

    assert_eq!(name, ("guest".to_owned(),));
}

#[test]
fn fetch_optional_returns_none_for_absent_rows() {
    let mut adapter = seeded();

    let query = SelectQuery::<Sqlite<Sqlite3_35>>::new()
        .from::<User>()
        .select((UserName,))
        .where_(Expr::column::<UserId>().eq(99i64));

    assert_eq!(adapter.fetch_optional(&query).expect("optional"), None);
}
