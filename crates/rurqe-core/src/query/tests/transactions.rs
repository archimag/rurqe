//! Transaction statements and the nested savepoint context.

use crate::expr::SqlVal;
use crate::query::*;

/// A transaction backend recording the statements it is asked to run.
#[derive(Default)]
struct MockTransactionBackend {
    statements: Vec<String>,
}

impl QueryBackend for MockTransactionBackend {
    type Error = &'static str;

    fn fetch_raw(&mut self, _query: &BuiltQuery) -> Result<Vec<Vec<SqlVal>>, Self::Error> {
        Ok(Vec::new())
    }

    fn execute_raw(&mut self, query: &BuiltQuery) -> Result<u64, Self::Error> {
        self.statements.push(query.sql.clone());
        Ok(0)
    }
}

impl TransactionBackend for MockTransactionBackend {}

#[test]
fn transaction_statements_render_expected_sql() {
    let mut backend = MockTransactionBackend::default();
    backend.begin(None).expect("begin");
    backend
        .begin(Some(IsolationLevel::Serializable))
        .expect("begin with isolation");
    let first = SavepointName::new("rurqe_sp_1").expect("a valid savepoint name");
    let second = SavepointName::new("rurqe_sp_2").expect("a valid savepoint name");
    backend.savepoint(&first).expect("savepoint");
    backend.release_savepoint(&first).expect("release");
    backend
        .rollback_to_savepoint(&second)
        .expect("rollback to savepoint");
    backend.commit().expect("commit");
    backend.rollback().expect("rollback");

    assert_eq!(
        backend.statements,
        vec![
            "BEGIN",
            "BEGIN ISOLATION LEVEL SERIALIZABLE",
            "SAVEPOINT rurqe_sp_1",
            "RELEASE SAVEPOINT rurqe_sp_1",
            "ROLLBACK TO SAVEPOINT rurqe_sp_2",
            "COMMIT",
            "ROLLBACK",
        ]
    );
}

#[test]
fn isolation_levels_render_canonical_clauses() {
    assert_eq!(IsolationLevel::ReadCommitted.as_str(), "READ COMMITTED");
    assert_eq!(IsolationLevel::RepeatableRead.as_str(), "REPEATABLE READ");
    assert_eq!(IsolationLevel::Serializable.as_str(), "SERIALIZABLE");
}

#[test]
fn transaction_context_hands_out_nested_savepoints() {
    let mut context = TransactionContext::new();
    assert_eq!(context.depth(), 0);
    assert!(!context.is_nested());
    assert_eq!(context.current_savepoint(), None);

    let outer = context.open_savepoint();
    let inner = context.open_savepoint();
    assert_ne!(outer, inner);
    assert_eq!(context.depth(), 2);
    assert!(context.is_nested());
    assert_eq!(context.current_savepoint(), Some(inner.as_str()));

    assert_eq!(context.close_savepoint(), Some(inner));
    assert_eq!(context.current_savepoint(), Some(outer.as_str()));
    assert_eq!(context.close_savepoint(), Some(outer));
    assert_eq!(context.close_savepoint(), None);
}
