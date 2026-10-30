//! The guard suite of the typed transaction scopes.

use super::*;

use crate::expr::SqlVal;
use crate::query::{BuiltQuery, QueryBackend};

/// A connection whose transactional statements are scripted by the test.
#[derive(Default)]
struct MockConnection {
    status: TransactionStatus<String>,
    statements: Vec<String>,
    fail_rollback: bool,
    fail_savepoint_rollback: bool,
}

impl QueryBackend for MockConnection {
    type Error = String;

    fn fetch_raw(&mut self, _query: &BuiltQuery) -> Result<Vec<Vec<SqlVal>>, Self::Error> {
        Ok(Vec::new())
    }

    fn execute_raw(&mut self, query: &BuiltQuery) -> Result<u64, Self::Error> {
        self.statements.push(query.sql.clone());
        Ok(0)
    }
}

impl TransactionBackend for MockConnection {
    fn rollback(&mut self) -> Result<(), Self::Error> {
        self.statements.push(String::from("ROLLBACK"));
        if self.fail_rollback {
            Err(String::from("rollback refused"))
        } else {
            Ok(())
        }
    }

    fn rollback_to_savepoint(&mut self, name: &SavepointName) -> Result<(), Self::Error> {
        self.statements
            .push(format!("ROLLBACK TO SAVEPOINT {name}"));
        if self.fail_savepoint_rollback {
            Err(String::from("savepoint rollback refused"))
        } else {
            Ok(())
        }
    }
}

impl TransactionalConnection for MockConnection {
    fn transaction_status(&mut self) -> &mut TransactionStatus<String> {
        &mut self.status
    }
}

#[test]
fn a_failed_rollback_poisons_the_connection() {
    let mut connection = MockConnection {
        fail_rollback: true,
        ..MockConnection::default()
    };

    drop(TransactionGuard::begin(&mut connection).expect("the transaction opens"));

    assert!(connection.status.is_poisoned());
    assert!(!connection.status.is_owned());
    assert!(
        connection.status.is_open(),
        "a refused rollback never closes the transaction"
    );
    assert_eq!(
        connection.statements,
        vec![String::from("BEGIN"), String::from("ROLLBACK")]
    );
}

#[test]
fn a_poisoned_connection_refuses_a_new_transaction() {
    let mut connection = MockConnection {
        fail_rollback: true,
        ..MockConnection::default()
    };
    drop(TransactionGuard::begin(&mut connection).expect("the transaction opens"));

    let refused = TransactionGuard::begin(&mut connection);

    assert_eq!(refused.err(), Some(String::from("rollback refused")));
    assert!(connection.status.is_poisoned());
    assert!(connection.status.is_open());
}

#[test]
fn a_poisoned_connection_recovers_once_the_rollback_succeeds() {
    let mut connection = MockConnection {
        fail_rollback: true,
        ..MockConnection::default()
    };
    drop(TransactionGuard::begin(&mut connection).expect("the transaction opens"));
    assert!(connection.status.is_poisoned());

    connection.fail_rollback = false;
    let guard = TransactionGuard::begin(&mut connection).expect("the connection recovers");
    assert!(guard.is_outer());
    assert!(!guard.status.is_poisoned());
    assert!(guard.status.is_open());
    guard.commit().expect("the recovered transaction commits");

    assert!(!connection.status.is_open());
    assert!(!connection.status.is_poisoned());
}

#[test]
fn a_failed_savepoint_rollback_poisons_the_connection() {
    let mut connection = MockConnection {
        fail_savepoint_rollback: true,
        ..MockConnection::default()
    };
    let mut outer = TransactionGuard::begin(&mut connection).expect("the transaction opens");
    drop(TransactionGuard::begin(&mut *outer).expect("the savepoint opens"));

    assert!(outer.status.is_poisoned());
    assert!(outer.status.is_owned());
    assert!(
        outer.status.is_open(),
        "the enclosing transaction stays open"
    );
    assert_eq!(
        outer.status.depth(),
        1,
        "a refused savepoint rollback keeps the savepoint recorded"
    );
}

#[test]
fn a_poisoned_connection_refuses_to_commit_the_enclosing_transaction() {
    let mut connection = MockConnection {
        fail_savepoint_rollback: true,
        ..MockConnection::default()
    };
    let mut outer = TransactionGuard::begin(&mut connection).expect("the transaction opens");
    drop(TransactionGuard::begin(&mut *outer).expect("the savepoint opens"));

    let refused = outer.commit();

    assert_eq!(
        refused.err(),
        Some(String::from("savepoint rollback refused")),
        "the refusal hands the error of the failed rollback back"
    );
    assert_eq!(
        connection.statements,
        vec![
            String::from("BEGIN"),
            String::from("SAVEPOINT rurqe_sp_1"),
            String::from("ROLLBACK TO SAVEPOINT rurqe_sp_1"),
            String::from("ROLLBACK"),
        ],
        "the abandoned work is never committed, and the guard forces the rollback"
    );
    assert!(!connection.status.is_open());
    assert!(!connection.status.is_poisoned());
}

#[test]
fn a_nested_retry_never_rolls_back_the_enclosing_transaction() {
    let mut connection = MockConnection {
        fail_savepoint_rollback: true,
        ..MockConnection::default()
    };
    let mut outer = TransactionGuard::begin(&mut connection).expect("the transaction opens");
    drop(TransactionGuard::begin(&mut *outer).expect("the savepoint opens"));
    assert!(outer.status.is_poisoned());
    assert!(outer.status.is_owned());
    let recorded = outer.statements.len();

    let refused = TransactionGuard::begin(&mut *outer);

    assert_eq!(
        refused.err(),
        Some(String::from("savepoint rollback refused"))
    );
    assert_eq!(
        outer.statements[recorded..],
        [String::from("ROLLBACK TO SAVEPOINT rurqe_sp_1")],
        "the retry closes only the abandoned savepoint"
    );
    assert!(outer.status.is_poisoned());
    assert!(outer.status.is_open());
    assert!(outer.status.is_owned());
    assert_eq!(outer.status.depth(), 1);

    let refused = outer.commit();

    assert_eq!(
        refused.err(),
        Some(String::from("savepoint rollback refused")),
        "the enclosing transaction reports the pending failure instead of committing"
    );
    assert_eq!(
        connection.statements,
        vec![
            String::from("BEGIN"),
            String::from("SAVEPOINT rurqe_sp_1"),
            String::from("ROLLBACK TO SAVEPOINT rurqe_sp_1"),
            String::from("ROLLBACK TO SAVEPOINT rurqe_sp_1"),
            String::from("ROLLBACK"),
        ],
        "no outermost rollback and no second BEGIN are issued before the outer drop"
    );
    assert!(!connection.status.is_open());
    assert!(!connection.status.is_poisoned());
    assert!(!connection.status.is_owned());
}

#[test]
fn a_nested_retry_that_heals_reopens_the_savepoint() {
    let mut connection = MockConnection {
        fail_savepoint_rollback: true,
        ..MockConnection::default()
    };
    let mut outer = TransactionGuard::begin(&mut connection).expect("the transaction opens");
    drop(TransactionGuard::begin(&mut *outer).expect("the savepoint opens"));
    assert!(outer.status.is_poisoned());

    outer.fail_savepoint_rollback = false;

    let nested = TransactionGuard::begin(&mut *outer).expect("the retry heals the savepoint");
    assert!(!nested.is_outer());
    assert!(!nested.status.is_poisoned());
    assert_eq!(nested.status.depth(), 1);
    nested.commit().expect("the fresh savepoint releases");

    assert!(!outer.status.is_poisoned());
    assert_eq!(outer.status.depth(), 0);
    outer.commit().expect("the outer transaction commits");

    assert_eq!(
        connection.statements,
        vec![
            String::from("BEGIN"),
            String::from("SAVEPOINT rurqe_sp_1"),
            String::from("ROLLBACK TO SAVEPOINT rurqe_sp_1"),
            String::from("ROLLBACK TO SAVEPOINT rurqe_sp_1"),
            String::from("RELEASE SAVEPOINT rurqe_sp_1"),
            String::from("SAVEPOINT rurqe_sp_2"),
            String::from("RELEASE SAVEPOINT rurqe_sp_2"),
            String::from("COMMIT"),
        ]
    );
    assert!(!connection.status.is_open());
}

#[test]
fn an_unowned_poison_recovers_the_whole_transaction() {
    let mut connection = MockConnection {
        fail_savepoint_rollback: true,
        fail_rollback: true,
        ..MockConnection::default()
    };
    {
        let mut outer = TransactionGuard::begin(&mut connection).expect("the transaction opens");
        drop(TransactionGuard::begin(&mut *outer).expect("the savepoint opens"));
    }

    assert!(connection.status.is_poisoned());
    assert!(!connection.status.is_owned());
    assert!(connection.status.is_open());
    assert_eq!(
        connection.status.depth(),
        1,
        "the abandoned savepoint stays recorded"
    );

    connection.fail_rollback = false;

    let guard = TransactionGuard::begin(&mut connection).expect("the connection recovers");
    assert!(guard.is_outer());
    assert!(guard.status.is_owned());
    assert!(!guard.status.is_poisoned());
    assert_eq!(guard.status.depth(), 0);
    guard.commit().expect("the recovered transaction commits");

    assert_eq!(
        connection.statements,
        vec![
            String::from("BEGIN"),
            String::from("SAVEPOINT rurqe_sp_1"),
            String::from("ROLLBACK TO SAVEPOINT rurqe_sp_1"),
            String::from("ROLLBACK"),
            String::from("ROLLBACK"),
            String::from("BEGIN"),
            String::from("COMMIT"),
        ],
        "ownership, not savepoint depth, selects the full recovery"
    );
}

#[test]
fn a_nested_block_rolls_back_to_its_savepoint() {
    let mut connection = MockConnection::default();
    let mut outer = TransactionGuard::begin(&mut connection).expect("the transaction opens");
    drop(TransactionGuard::begin(&mut *outer).expect("the savepoint opens"));

    assert!(!outer.status.is_poisoned());
    assert!(outer.status.is_open());
    assert_eq!(outer.status.depth(), 0);

    outer.commit().expect("the outer transaction commits");
    assert!(!connection.status.is_open());
}

#[test]
fn begin_with_renders_the_requested_isolation_level() {
    for (level, expected) in [
        (
            IsolationLevel::ReadCommitted,
            "BEGIN ISOLATION LEVEL READ COMMITTED",
        ),
        (
            IsolationLevel::RepeatableRead,
            "BEGIN ISOLATION LEVEL REPEATABLE READ",
        ),
        (
            IsolationLevel::Serializable,
            "BEGIN ISOLATION LEVEL SERIALIZABLE",
        ),
    ] {
        let mut connection = MockConnection::default();
        let guard = TransactionGuard::begin_with(&mut connection, Some(level))
            .expect("the transaction opens");

        assert!(guard.is_outer());
        assert_eq!(guard.statements, vec![String::from(expected)]);
        assert!(guard.status.is_open());
    }
}

#[test]
fn begin_without_an_isolation_level_renders_a_plain_begin() {
    let mut connection = MockConnection::default();
    let guard = TransactionGuard::begin(&mut connection).expect("the transaction opens");
    assert_eq!(guard.statements, vec![String::from("BEGIN")]);
    drop(guard);

    let mut connection = MockConnection::default();
    let guard =
        TransactionGuard::begin_with(&mut connection, None).expect("the transaction opens");
    assert_eq!(guard.statements, vec![String::from("BEGIN")]);
}

#[test]
fn a_nested_block_ignores_the_requested_isolation_level() {
    let mut connection = MockConnection::default();
    let mut outer = TransactionGuard::begin(&mut connection).expect("the transaction opens");
    drop(
        TransactionGuard::begin_with(&mut *outer, Some(IsolationLevel::Serializable))
            .expect("the savepoint opens"),
    );

    assert_eq!(
        outer.statements,
        vec![
            String::from("BEGIN"),
            String::from("SAVEPOINT rurqe_sp_1"),
            String::from("ROLLBACK TO SAVEPOINT rurqe_sp_1"),
            String::from("RELEASE SAVEPOINT rurqe_sp_1"),
        ]
    );
    assert!(
        !outer
            .statements
            .iter()
            .any(|statement| statement.contains("ISOLATION LEVEL")),
        "a savepoint carries no isolation clause"
    );
    assert!(outer.status.is_open());
    assert_eq!(outer.status.depth(), 0);
}

#[test]
fn a_committed_isolation_transaction_renders_begin_then_commit() {
    let mut connection = MockConnection::default();
    let guard =
        TransactionGuard::begin_with(&mut connection, Some(IsolationLevel::Serializable))
            .expect("the transaction opens");
    guard.commit().expect("the transaction commits");

    assert_eq!(
        connection.statements,
        vec![
            String::from("BEGIN ISOLATION LEVEL SERIALIZABLE"),
            String::from("COMMIT"),
        ]
    );
    assert!(!connection.status.is_open());
    assert!(!connection.status.is_poisoned());
    assert_eq!(connection.status.depth(), 0);
}
