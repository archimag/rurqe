//! End-to-end execution tests for the `rusqlite` adapter against in-memory SQLite.

use rurqe_sqlite::rurqe_core::dml::{ColumnAssign, InsertQuery};
use rurqe_sqlite::rurqe_core::expr::Expr;
use rurqe_sqlite::rurqe_core::query::{ExecutionError, QueryBackendExt, SelectQuery};
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

/// Minimal error type used by the transaction closures in the tests below.
#[derive(Debug)]
struct TestError;

impl From<rurqe_sqlite::rusqlite::Error> for TestError {
    fn from(_: rurqe_sqlite::rusqlite::Error) -> Self {
        TestError
    }
}

#[test]
fn transaction_commits_work_on_success() {
    let connection = Connection::open_in_memory().expect("open in-memory database");
    connection
        .execute_batch(CREATE_USERS)
        .expect("create users table");
    let mut adapter = SqliteConnection::new(connection);

    adapter
        .transaction(|conn| -> Result<(), TestError> {
            conn.execute_batch("INSERT INTO users (id, name) VALUES (1, 'root')")?;
            Ok(())
        })
        .expect("transaction commits");

    let count: i64 = adapter
        .query_row("SELECT COUNT(*) FROM users", [], |row| row.get(0))
        .expect("count rows");
    assert_eq!(count, 1);
}

#[test]
fn transaction_rolls_back_on_error() {
    let connection = Connection::open_in_memory().expect("open in-memory database");
    connection
        .execute_batch(CREATE_USERS)
        .expect("create users table");
    let mut adapter = SqliteConnection::new(connection);

    let outcome = adapter.transaction(|conn| -> Result<(), TestError> {
        conn.execute_batch("INSERT INTO users (id, name) VALUES (1, 'root')")?;
        Err(TestError)
    });
    assert!(outcome.is_err());

    let count: i64 = adapter
        .query_row("SELECT COUNT(*) FROM users", [], |row| row.get(0))
        .expect("count rows");
    assert_eq!(count, 0);
}

#[test]
fn nested_transaction_rolls_back_to_its_savepoint() {
    let connection = Connection::open_in_memory().expect("open in-memory database");
    connection
        .execute_batch(CREATE_USERS)
        .expect("create users table");
    let mut adapter = SqliteConnection::new(connection);

    adapter
        .transaction(|conn| -> Result<(), TestError> {
            conn.execute_batch("INSERT INTO users (id, name) VALUES (1, 'root')")?;
            let inner = conn.transaction(|inner| -> Result<(), TestError> {
                inner.execute_batch("INSERT INTO users (id, name) VALUES (2, 'guest')")?;
                Err(TestError)
            });
            assert!(inner.is_err());
            Ok(())
        })
        .expect("outer transaction commits");

    let count: i64 = adapter
        .query_row("SELECT COUNT(*) FROM users", [], |row| row.get(0))
        .expect("count rows");
    assert_eq!(count, 1);
    let name: String = adapter
        .query_row("SELECT name FROM users", [], |row| row.get(0))
        .expect("select name");
    assert_eq!(name, "root");
}

#[test]
fn nested_transaction_releases_its_savepoint_on_success() {
    let connection = Connection::open_in_memory().expect("open in-memory database");
    connection
        .execute_batch(CREATE_USERS)
        .expect("create users table");
    let mut adapter = SqliteConnection::new(connection);

    adapter
        .transaction(|conn| -> Result<(), TestError> {
            conn.execute_batch("INSERT INTO users (id, name) VALUES (1, 'root')")?;
            conn.transaction(|inner| -> Result<(), TestError> {
                inner.execute_batch("INSERT INTO users (id, name) VALUES (2, 'guest')")?;
                Ok(())
            })?;
            Ok(())
        })
        .expect("outer transaction commits");

    let count: i64 = adapter
        .query_row("SELECT COUNT(*) FROM users", [], |row| row.get(0))
        .expect("count rows");
    assert_eq!(count, 2);
}

/// Counts the rows of `users` through a helper naming the concrete error type.
///
/// The signature spells out `ExecutionError<rusqlite::Error>` instead of erasing
/// the failure into `Box<dyn std::error::Error>`, so the caller keeps exhaustive
/// pattern matching and no allocation happens on the failure path.
fn count_users(
    adapter: &mut SqliteConnection,
) -> Result<usize, ExecutionError<rurqe_sqlite::rusqlite::Error>> {
    let query = SelectQuery::<Sqlite<Sqlite3_35>>::new()
        .from::<User>()
        .select((UserId,));
    Ok(adapter.fetch_all(&query)?.len())
}

/// Reads one user name through the same concrete error type.
fn user_name(
    adapter: &mut SqliteConnection,
    id: i64,
) -> Result<String, ExecutionError<rurqe_sqlite::rusqlite::Error>> {
    let query = SelectQuery::<Sqlite<Sqlite3_35>>::new()
        .from::<User>()
        .select((UserName,))
        .where_(Expr::column::<UserId>().eq(id));
    let (name,) = adapter.fetch_one(&query)?;
    Ok(name)
}

#[test]
fn typed_execution_errors_are_inspectable_without_boxing() {
    let mut adapter = seeded();

    assert_eq!(count_users(&mut adapter).expect("counting succeeds"), 2);
    assert_eq!(user_name(&mut adapter, 2).expect("user exists"), "guest");

    // A single-row lookup matching nothing reports the typed `NoRows` failure.
    let missing = user_name(&mut adapter, 99).expect_err("no matching row");
    assert!(missing.is_not_found());
    assert!(!missing.is_multiple_rows());
    assert!(!missing.is_row_decode());
    assert!(missing.as_backend().is_none());
    assert!(missing.into_backend().is_none());

    // A single-row lookup matching several rows keeps its own failure kind.
    let all_users = SelectQuery::<Sqlite<Sqlite3_35>>::new()
        .from::<User>()
        .select((UserId,));
    let duplicated = adapter.fetch_one(&all_users).expect_err("two rows match");
    assert!(duplicated.is_multiple_rows());
    assert!(!duplicated.is_not_found());

    // Driver failures stay typed and reachable through `as_backend`.
    let broken = SelectQuery::<Sqlite<Sqlite3_35>>::new()
        .from::<User>()
        .select((UserId, Expr::<i64>::raw("missing_function()", vec![])));
    let failed = adapter
        .fetch_all(&broken)
        .expect_err("the driver rejects the unknown function");
    assert!(!failed.is_not_found());
    assert!(!failed.is_row_decode());
    let backend = failed.as_backend().expect("driver error is preserved");
    // The prepare-time failure carries a concrete driver variant that depends
    // on the bundled SQLite version, so the driver message is inspected instead
    // of a single variant.
    assert!(backend.to_string().contains("missing_function"), "unexpected error: {backend}");

    let owned = failed.into_backend().expect("driver error is owned");
    assert!(owned.to_string().contains("missing_function"), "unexpected error: {owned}");
}
