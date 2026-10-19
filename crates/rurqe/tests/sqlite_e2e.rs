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

/// Counts the rows of `users` through the pinned `Result<T>` alias.
///
/// The signature names the concrete error type of the SQLite dialect instead of
/// erasing it into `Box<dyn std::error::Error>`, so callers keep exhaustive
/// pattern matching over the failure.
fn count_users(db: &mut SqliteConnection) -> rurqe::sqlite::Result<usize> {
    let query = select_from::<Sqlite<Sqlite3_35>, User>().select((UserId,));
    let rows = db.fetch_all(&query)?;
    Ok(rows.len())
}

/// Reads one user name through the explicit two-parameter form of the alias.
fn user_name(
    db: &mut SqliteConnection,
    id: i64,
) -> rurqe::sqlite::Result<String, rurqe::sqlite::Error> {
    let query = select_from::<Sqlite<Sqlite3_35>, User>()
        .select((UserName,))
        .where_(Expr::column::<UserId>().eq(id));
    let (name,) = db.fetch_one(&query)?;
    Ok(name)
}

/// Statically asserts that the facade aliases name the shared execution error.
///
/// The pinned `Result<T>` alias and the `Error` alias must stay the very same
/// type as `ExecutionError<rusqlite::Error>`, so a helper written against the
/// alias of the dialect module and a helper written against the core type stay
/// interchangeable, and the failure is never widened into an untyped box.
fn assert_pinned_aliases_match_the_shared_error(
    _: rurqe::sqlite::Result<()>,
    _: core::result::Result<(), ExecutionError<rurqe::sqlite::rusqlite::Error>>,
) {
}

#[test]
fn typed_sqlite_errors_are_inspectable_through_the_facade() {
    assert_pinned_aliases_match_the_shared_error(Ok(()), Ok(()));

    let mut db = database();

    assert_eq!(count_users(&mut db).expect("counting succeeds"), 0);

    // A `fetch_one` on an empty table reports the missing row through a typed
    // error the caller inspects without matching the enum by hand.
    let missing = user_name(&mut db, 1).expect_err("the table is empty");
    assert!(missing.is_not_found());
    assert!(!missing.is_multiple_rows());
    assert!(!missing.is_row_decode());
    assert!(missing.as_backend().is_none());

    let insert_root = insert_into::<Sqlite<Sqlite3_35>, User>().values((
        UserId::set(1i64),
        UserName::set("root"),
        UserActive::set(true),
        UserScore::set(42i32),
    ));
    db.execute(&insert_root).expect("insert root");

    let insert_guest = insert_into::<Sqlite<Sqlite3_35>, User>().values((
        UserId::set(2i64),
        UserName::set("guest"),
        UserActive::set(false),
        UserScore::set(7i32),
    ));
    db.execute(&insert_guest).expect("insert guest");

    assert_eq!(count_users(&mut db).expect("counting succeeds"), 2);
    assert_eq!(user_name(&mut db, 1).expect("user exists"), "root");

    // A single-row lookup matching several rows keeps its own typed failure.
    let all_users = select_from::<Sqlite<Sqlite3_35>, User>().select((UserId,));
    let duplicated = db.fetch_one(&all_users).expect_err("two rows match");
    assert!(duplicated.is_multiple_rows());
    assert!(!duplicated.is_not_found());

    // Driver failures stay typed and reachable through `as_backend`.
    let broken = select_from::<Sqlite<Sqlite3_35>, User>()
        .select((UserId, Expr::<i64>::raw("missing_function()", vec![])));
    let failed = db
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
