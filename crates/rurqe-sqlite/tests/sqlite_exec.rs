//! End-to-end execution tests for the `rusqlite` adapter against in-memory SQLite.

use rurqe_sqlite::rurqe_core::dml::InsertQuery;
use rurqe_sqlite::rurqe_core::expr::{Expr, SqlVal};
use rurqe_sqlite::rurqe_core::query::{
    ExecutionError, QueryBackendExt, SavepointName, SelectQuery, TransactionBackend,
    TransactionalConnection,
};
use rurqe_sqlite::rurqe_core::table;
use rurqe_sqlite::rusqlite::Connection;
use rurqe_sqlite::{Sqlite, Sqlite3_35, SqliteConnection};

table!(
    /// The `users` table.
    User => "users" {
        UserId => "id": i64,
        UserName => "name": String,
    }
);

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
        .execute(
            "INSERT INTO users (id, name) VALUES (?1, ?2)",
            (1i64, "root"),
        )
        .expect("insert first row");
    connection
        .execute(
            "INSERT INTO users (id, name) VALUES (?1, ?2)",
            (2i64, "guest"),
        )
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

#[test]
fn ilike_filters_case_insensitively_through_lower_folding() {
    let mut adapter = seeded();

    let query = SelectQuery::<Sqlite<Sqlite3_35>>::new()
        .from::<User>()
        .select((UserName,))
        .where_(Expr::column::<UserName>().ilike("R%"))
        .order_by_asc(UserId);

    // SQLite has no `ILIKE`, so the predicate renders as
    // `LOWER("name") LIKE LOWER(?)`: `root` matches through the case folding
    // alone, while the case-sensitive `LIKE 'R%'` would match no seeded row.
    let rows = adapter.fetch_all(&query).expect("fetch all succeeds");

    assert_eq!(rows, vec![("root".to_owned(),)]);
}

#[test]
fn an_array_parameter_binds_as_a_json_document_and_decodes_back() {
    let mut adapter = seeded();

    // SQLite has no array column, so an array parameter binds as the JSON
    // document of its elements. The very same values decode back into the
    // element type of the array, which is how an aggregated row set travels
    // through the driver: a parameter rendered in PostgreSQL's `{...}` spelling
    // would bind text no decoder can read back.
    let names = vec![
        SqlVal::Text("root".to_owned()),
        SqlVal::Text("guest".to_owned()),
    ];
    let numbers = vec![SqlVal::I64(1), SqlVal::I64(2)];
    let ratios = vec![SqlVal::F64(0.5), SqlVal::F64(1.5)];

    let query = SelectQuery::<Sqlite<Sqlite3_35>>::new()
        .from::<User>()
        .select((
            Expr::<Vec<String>>::raw("?", vec![SqlVal::Array(names)])
                .expect("a template with one placeholder"),
            Expr::<Vec<i64>>::raw("?", vec![SqlVal::Array(numbers)])
                .expect("a template with one placeholder"),
            Expr::<Vec<f64>>::raw("?", vec![SqlVal::Array(ratios)])
                .expect("a template with one placeholder"),
        ))
        .where_(Expr::column::<UserId>().eq(1i64));

    let (decoded_names, decoded_numbers, decoded_ratios) = adapter
        .fetch_one(&query)
        .expect("the bound arrays decode back");

    assert_eq!(decoded_names, vec!["root".to_owned(), "guest".to_owned()]);
    assert_eq!(decoded_numbers, vec![1i64, 2]);
    // A finite double writes its number notation into the document, so the
    // value the caller bound is exactly the value that decodes back.
    assert_eq!(decoded_ratios, vec![0.5, 1.5]);
}

#[test]
fn a_nan_parameter_is_refused_instead_of_binding_as_null() {
    let mut adapter = seeded();

    // SQLite has no storage class for NaN: a bound NaN double becomes NULL
    // without an error, so the adapter refuses the parameter instead of
    // silently substituting a value on the write path.
    let nan = Expr::<f64>::raw("?", vec![SqlVal::F64(f64::NAN)])
        .expect("a template with one placeholder");
    let query = SelectQuery::<Sqlite<Sqlite3_35>>::new()
        .from::<User>()
        .select((nan,))
        .where_(Expr::column::<UserId>().eq(1i64));

    let failed = adapter
        .fetch_one(&query)
        .expect_err("a NaN parameter has no SQLite representation");

    let backend = failed.as_backend().expect("the refusal is a driver error");
    match backend {
        rurqe_sqlite::rusqlite::Error::ToSqlConversionFailure(cause) => {
            assert!(
                cause.to_string().contains("NaN"),
                "unexpected cause: {cause}"
            );
        }
        other => panic!("unexpected error: {other}"),
    }
}

#[test]
fn a_non_finite_array_element_is_refused_instead_of_binding_as_json_null() {
    // The JSON document of an array parameter carries no notation for a
    // non-finite number, so the element is refused instead of being written as
    // `null`: a top-level infinity keeps binding as a REAL and a top-level NaN
    // is refused by the scalar path, while the same value inside an array would
    // otherwise reach the database as a value the caller never carried. Nested
    // arrays travel through the same recursive arm, so the refusal holds at
    // every depth.
    let cases = [
        (SqlVal::Array(vec![SqlVal::F64(f64::NAN)]), f64::NAN),
        (SqlVal::Array(vec![SqlVal::F64(f64::INFINITY)]), f64::INFINITY),
        (
            SqlVal::Array(vec![SqlVal::F64(f64::NEG_INFINITY)]),
            f64::NEG_INFINITY,
        ),
        (
            SqlVal::Array(vec![SqlVal::Array(vec![SqlVal::F64(f64::NAN)])]),
            f64::NAN,
        ),
    ];

    for (parameter, offender) in cases {
        let mut adapter = seeded();

        let element = Expr::<Vec<f64>>::raw("?", vec![parameter])
            .expect("a template with one placeholder");
        let query = SelectQuery::<Sqlite<Sqlite3_35>>::new()
            .from::<User>()
            .select((element,))
            .where_(Expr::column::<UserId>().eq(1i64));

        let failed = adapter
            .fetch_one(&query)
            .expect_err("a non-finite array element has no JSON notation");

        let backend = failed.as_backend().expect("the refusal is a driver error");
        match backend {
            rurqe_sqlite::rusqlite::Error::ToSqlConversionFailure(cause) => {
                let message = cause.to_string();
                assert!(
                    message.contains(&format!("({offender})")),
                    "the cause names the value: {message}"
                );
                assert!(
                    message.contains("array parameter"),
                    "the cause names the array context: {message}"
                );
            }
            other => panic!("unexpected error: {other}"),
        }
    }
}

#[test]
fn infinities_bind_faithfully_because_sqlite_stores_them() {
    let mut adapter = seeded();

    // SQLite stores infinities as ordinary REAL values, so they round-trip
    // unchanged and need no refusal of their own.
    let positive = Expr::<f64>::raw("?", vec![SqlVal::F64(f64::INFINITY)])
        .expect("a template with one placeholder");
    let negative = Expr::<f64>::raw("?", vec![SqlVal::F64(f64::NEG_INFINITY)])
        .expect("a template with one placeholder");
    let query = SelectQuery::<Sqlite<Sqlite3_35>>::new()
        .from::<User>()
        .select((positive, negative))
        .where_(Expr::column::<UserId>().eq(1i64));

    let (infinity, negative_infinity) = adapter
        .fetch_one(&query)
        .expect("infinities round-trip through SQLite");

    assert!(infinity.is_infinite() && infinity.is_sign_positive());
    assert!(negative_infinity.is_infinite() && negative_infinity.is_sign_negative());
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
#[test]
fn savepoint_statements_accept_a_validated_name() {
    let connection = Connection::open_in_memory().expect("open in-memory database");
    connection
        .execute_batch(CREATE_USERS)
        .expect("create users table");
    let mut adapter = SqliteConnection::new(connection);
    let savepoint = SavepointName::new("rurqe_sp_9").expect("a valid savepoint name");

    adapter.begin(None).expect("the transaction opens");
    adapter
        .execute_batch("INSERT INTO users (id, name) VALUES (1, 'root')")
        .expect("insert outside the savepoint");

    // The adapter renders a savepoint name as a quoted identifier wherever it
    // concatenates one, and a quoted name parses to the same savepoint the
    // shared `SAVEPOINT` statement creates, so the validated name keeps
    // addressing one savepoint across creation, rollback, and release.
    adapter.savepoint(&savepoint).expect("the savepoint opens");
    adapter
        .execute_batch("INSERT INTO users (id, name) VALUES (2, 'guest')")
        .expect("insert inside the savepoint");
    adapter
        .rollback_to_savepoint(&savepoint)
        .expect("the savepoint rolls back");
    adapter
        .release_savepoint(&savepoint)
        .expect("the savepoint releases");
    adapter.commit().expect("the transaction commits");

    let count: i64 = adapter
        .query_row("SELECT COUNT(*) FROM users", [], |row| row.get(0))
        .expect("count rows");
    assert_eq!(count, 1, "the savepoint rollback discarded the inner insert");

    let surviving: String = adapter
        .query_row("SELECT name FROM users", [], |row| row.get(0))
        .expect("select name");
    assert_eq!(surviving, "root");
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
        .select((
            UserId,
            Expr::<i64>::raw("missing_function()", vec![])
                .expect("a template without placeholders"),
        ));
    let failed = adapter
        .fetch_all(&broken)
        .expect_err("the driver rejects the unknown function");
    assert!(!failed.is_not_found());
    assert!(!failed.is_row_decode());
    let backend = failed.as_backend().expect("driver error is preserved");
    // The prepare-time failure carries a concrete driver variant that depends
    // on the bundled SQLite version, so the driver message is inspected instead
    // of a single variant.
    assert!(
        backend.to_string().contains("missing_function"),
        "unexpected error: {backend}"
    );

    let owned = failed.into_backend().expect("driver error is owned");
    assert!(
        owned.to_string().contains("missing_function"),
        "unexpected error: {owned}"
    );
}

#[test]
fn a_raw_commit_through_the_handle_costs_only_its_own_call_its_error() {
    let connection = Connection::open_in_memory().expect("open in-memory database");
    connection
        .execute_batch(CREATE_USERS)
        .expect("create users table");
    let mut adapter = SqliteConnection::new(connection);

    let outcome = adapter.transaction(|conn| -> Result<(), rurqe_sqlite::rusqlite::Error> {
        conn.execute_batch("INSERT INTO users (id, name) VALUES (1, 'root')")?;
        // The raw handle is a supported escape hatch: committing through it
        // closes the transaction the typed scope still believes it owns.
        conn.execute_batch("COMMIT")?;
        Ok(())
    });

    let failed = outcome.expect_err("the scope's own commit finds no transaction");
    assert!(
        failed.to_string().contains("cannot commit"),
        "unexpected error: {failed}"
    );
    assert!(
        !adapter.transaction_status().is_poisoned(),
        "a desynchronized commit costs its own call its error, nothing else"
    );
    assert!(!adapter.transaction_status().is_open());

    let count: i64 = adapter
        .query_row("SELECT COUNT(*) FROM users", [], |row| row.get(0))
        .expect("count rows");
    assert_eq!(count, 1, "the raw commit persisted the row");

    adapter
        .transaction(|conn| -> Result<(), TestError> {
            conn.execute_batch("INSERT INTO users (id, name) VALUES (2, 'guest')")?;
            Ok(())
        })
        .expect("a fresh typed transaction opens");
}

#[test]
fn a_raw_rollback_inside_a_typed_scope_leaves_the_connection_clean() {
    let connection = Connection::open_in_memory().expect("open in-memory database");
    connection
        .execute_batch(CREATE_USERS)
        .expect("create users table");
    let mut adapter = SqliteConnection::new(connection);

    let outcome = adapter.transaction(|conn| -> Result<(), TestError> {
        conn.execute_batch("INSERT INTO users (id, name) VALUES (1, 'root')")?;
        // The raw handle is a supported escape hatch: rolling the transaction
        // back through it leaves the guard a rollback without a transaction.
        conn.execute_batch("ROLLBACK")?;
        Err(TestError)
    });

    assert!(outcome.is_err());
    assert!(
        !adapter.transaction_status().is_poisoned(),
        "the refused rollback is benign: the driver observes no open transaction"
    );
    assert!(!adapter.transaction_status().is_open());

    let count: i64 = adapter
        .query_row("SELECT COUNT(*) FROM users", [], |row| row.get(0))
        .expect("count rows");
    assert_eq!(count, 0, "the raw rollback discarded the row");

    adapter
        .transaction(|conn| -> Result<(), TestError> {
            conn.execute_batch("INSERT INTO users (id, name) VALUES (2, 'guest')")?;
            Ok(())
        })
        .expect("a fresh typed transaction opens");
}

#[test]
fn a_raw_rollback_inside_a_nested_scope_does_not_poison_the_bookkeeping() {
    let connection = Connection::open_in_memory().expect("open in-memory database");
    connection
        .execute_batch(CREATE_USERS)
        .expect("create users table");
    let mut adapter = SqliteConnection::new(connection);

    let outcome = adapter.transaction(|conn| -> Result<(), TestError> {
        conn.execute_batch("INSERT INTO users (id, name) VALUES (1, 'root')")?;
        let inner = conn.transaction(|inner| -> Result<(), TestError> {
            inner.execute_batch("INSERT INTO users (id, name) VALUES (2, 'guest')")?;
            // The raw handle closes the transaction the savepoint belongs to,
            // so the nested guard finds neither a savepoint to roll back to
            // nor one to release.
            inner.execute_batch("ROLLBACK")?;
            Err(TestError)
        });

        assert!(
            inner.is_err(),
            "the closure error of the nested block travels back"
        );

        // The raw rollback took the enclosing transaction with it, so the
        // enclosing scope cannot commit any more either.
        Err(TestError)
    });

    assert!(outcome.is_err());
    assert!(
        !adapter.transaction_status().is_poisoned(),
        "the refused savepoint statements are benign: no transaction is open"
    );
    assert!(!adapter.transaction_status().is_open());
    assert_eq!(adapter.transaction_status().depth(), 0);

    adapter
        .transaction(|conn| -> Result<(), TestError> {
            conn.execute_batch("INSERT INTO users (id, name) VALUES (3, 'admin')")?;
            Ok(())
        })
        .expect("a fresh typed transaction opens");
}

/// Asserts that every bare statement entry refuses work on `connection`.
///
/// The refusal travels through the crate's adapter-refusal channel — a driver
/// conversion failure carrying the recorded transaction-control failure and the
/// recovery the next typed transaction entry performs — so the assertions pin
/// the contract instead of the driver's exact error variant.
fn assert_refuses_bare_statements(connection: &mut SqliteConnection) {
    let statement = SelectQuery::<Sqlite<Sqlite3_35>>::new()
        .from::<User>()
        .select((UserId, UserName));
    let insert = InsertQuery::<Sqlite<Sqlite3_35>, User>::new()
        .values((UserId::set(2i64), UserName::set("guest")));

    let refusals = [
        connection
            .fetch_all(&statement)
            .expect_err("fetch_raw refuses a poisoned connection"),
        connection
            .fetch_optional(&statement)
            .expect_err("fetch_raw_limited refuses a poisoned connection"),
        connection
            .fetch_one(&statement)
            .expect_err("fetch_raw_limited refuses a poisoned connection"),
        connection
            .execute(&insert)
            .expect_err("execute_raw refuses a poisoned connection"),
    ];

    for failure in refusals {
        let backend = failure.as_backend().expect("the refusal is an adapter error");
        match backend {
            rurqe_sqlite::rusqlite::Error::ToSqlConversionFailure(cause) => {
                let message = cause.to_string();
                assert!(
                    message.contains("refused transaction-control statement"),
                    "the refusal names the recorded failure: {message}"
                );
                assert!(
                    message.contains("the next typed transaction"),
                    "the refusal names the recovery: {message}"
                );
            }
            other => panic!("unexpected error: {other}"),
        }
    }
}

#[test]
fn a_poisoned_connection_refuses_bare_statements_until_the_transaction_is_closed() {
    let connection = Connection::open_in_memory().expect("open in-memory database");
    connection
        .execute_batch(CREATE_USERS)
        .expect("create users table");
    let mut adapter = SqliteConnection::new(connection);

    // A refused transaction-control statement is the only way into the poisoned
    // state, and SQLite rolls a transaction back whenever the database file is
    // still untouched, so the refusal is provoked where the engine does raise
    // one: the raw handle releases the savepoint of a nested block, which makes
    // the guard's own release and its fallback rollback fail while the
    // enclosing transaction stays open.
    let outcome = adapter.transaction(|conn| -> Result<(), TestError> {
        conn.execute_batch("INSERT INTO users (id, name) VALUES (1, 'root')")?;

        let nested = conn.transaction(|inner| -> Result<(), TestError> {
            inner.execute_batch("INSERT INTO users (id, name) VALUES (2, 'guest')")?;
            inner.execute_batch("RELEASE SAVEPOINT rurqe_sp_1")?;
            Ok(())
        });
        assert!(nested.is_err(), "the guard lost the savepoint of the nested block");

        // The transaction stays open, so the work of the failed block may still
        // be pending: every bare statement entry refuses before the driver sees
        // the statement.
        assert!(
            conn.is_poisoned(),
            "the refused savepoint statement poisons the adapter"
        );
        assert_refuses_bare_statements(conn);

        Ok(())
    });

    // The pending failure supersedes the successful closure: the enclosing
    // block refuses to commit, and its own rollback closes the transaction,
    // which discards both inserts and clears the record.
    assert!(
        outcome.is_err(),
        "the enclosing block refuses the pending failure"
    );
    assert!(
        !adapter.is_poisoned(),
        "closing the transaction clears the poison record"
    );
    assert!(!adapter.transaction_status().is_open());

    let statement = SelectQuery::<Sqlite<Sqlite3_35>>::new()
        .from::<User>()
        .select((UserId, UserName));
    let rows = adapter
        .fetch_all(&statement)
        .expect("the connection serves statements again");
    assert!(
        rows.is_empty(),
        "the enclosing rollback discarded both inserts"
    );

    adapter
        .transaction(|conn| -> Result<(), TestError> {
            conn.execute_batch("INSERT INTO users (id, name) VALUES (3, 'admin')")?;
            Ok(())
        })
        .expect("a fresh typed transaction opens");

    let rows = adapter.fetch_all(&statement).expect("fetch all succeeds");
    assert_eq!(rows, vec![(3i64, "admin".to_owned())]);
}