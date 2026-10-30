//! Query execution: the synchronous and asynchronous backend helpers.

use super::{
    async_user_query, block_on, spawnable, MockAsyncBackend, MockBackend, MockDialect, Order,
    OrderId, OrderTotal, User, UserId, UserName,
};
use crate::dml::insert::InsertedSelect;
use crate::dml::{ColumnAssign, DeleteQuery, Filtered, HasAssignments, InsertQuery, UpdateQuery};
use crate::expr::{Expr, SqlVal};
use crate::query::*;

#[test]
fn fetch_all_renders_the_query_and_decodes_rows() {
    let query = SelectQuery::<MockDialect>::new()
        .from::<User>()
        .select((UserId, UserName))
        .where_(Expr::column::<UserId>().gt(10i64));
    let query_built = query.build().expect("valid query");

    let mut backend = MockBackend {
        rows: vec![
            vec![SqlVal::I64(11), SqlVal::Text("root".to_owned())],
            vec![SqlVal::I64(12), SqlVal::Text("guest".to_owned())],
        ],
        ..MockBackend::default()
    };

    let rows = backend.fetch_all(&query).expect("rows decode");

    assert_eq!(
        rows,
        vec![(11i64, "root".to_owned()), (12i64, "guest".to_owned())]
    );
    assert_eq!(backend.executed, vec![query_built]);
}

#[test]
fn fetch_one_reports_missing_and_duplicate_rows() {
    let query = SelectQuery::<MockDialect>::new()
        .from::<User>()
        .select((UserId,));

    let mut empty = MockBackend::default();
    assert_eq!(empty.fetch_one(&query), Err(ExecutionError::NoRows));

    let mut many = MockBackend {
        rows: vec![vec![SqlVal::I64(1)], vec![SqlVal::I64(2)]],
        ..MockBackend::default()
    };
    assert_eq!(
        many.fetch_one(&query),
        Err(ExecutionError::MultipleRows { count: 2 })
    );

    let mut single = MockBackend {
        rows: vec![vec![SqlVal::I64(7)]],
        ..MockBackend::default()
    };
    assert_eq!(single.fetch_one(&query), Ok((7i64,)));
}

#[test]
fn fetch_optional_returns_none_for_absent_rows() {
    let query = SelectQuery::<MockDialect>::new()
        .from::<User>()
        .select((UserId,));

    let mut backend = MockBackend::default();
    assert_eq!(backend.fetch_optional(&query), Ok(None));

    backend.rows = vec![vec![SqlVal::I64(3)]];
    assert_eq!(backend.fetch_optional(&query), Ok(Some((3i64,))));
}

#[test]
fn fetch_all_reports_the_row_that_fails_to_decode() {
    let query = SelectQuery::<MockDialect>::new()
        .from::<User>()
        .select((UserId,));

    let mut backend = MockBackend {
        rows: vec![vec![SqlVal::Text("oops".to_owned())]],
        ..MockBackend::default()
    };

    assert_eq!(
        backend.fetch_all(&query),
        Err(ExecutionError::RowDecode {
            row_index: 0,
            source: crate::row::RowDecodeError::TypeMismatch {
                target: std::any::type_name::<i64>(),
                found: "text",
            },
        })
    );
}

#[test]
fn execute_reports_affected_rows_without_decoding() {
    let query =
        InsertQuery::<MockDialect, User>::new().values((UserId::set(1i64), UserName::set("root")));

    let mut backend = MockBackend {
        affected: 1,
        ..MockBackend::default()
    };

    assert_eq!(backend.execute(&query), Ok(1));
    assert_eq!(
        backend.executed,
        vec![query.build_query().expect("valid query")]
    );
}

#[test]
fn executable_queries_expose_built_sql_and_row_type() {
    let insert =
        InsertQuery::<MockDialect, User>::new().values((UserId::set(1i64), UserName::set("root")));
    assert_eq!(insert.build_query(), insert.build());

    fn assert_row<Q: ExecutableQuery<Row = (i64, String)>>() {}

    assert_row::<SelectQuery<MockDialect, User, (i64, String), HasProjection>>();
    assert_row::<InsertQuery<MockDialect, User, InsertedSelect<(i64, String)>, (i64, String)>>();
    assert_row::<UpdateQuery<MockDialect, User, Filtered, (i64, String), User, HasAssignments>>();
    assert_row::<DeleteQuery<MockDialect, User, Filtered, (i64, String)>>();
}

#[test]
fn execution_errors_render_actionable_messages() {
    assert_eq!(
        ExecutionError::<&str>::Backend("connection lost").to_string(),
        "database backend error: connection lost"
    );
    assert_eq!(
        ExecutionError::<&str>::RowDecode {
            row_index: 2,
            source: crate::row::RowDecodeError::ArityMismatch {
                expected: 2,
                actual: 1,
            },
        }
        .to_string(),
        "failed to decode row 2: cannot decode a row of 1 values into a row type of 2 values"
    );
    assert_eq!(
        ExecutionError::<&str>::NoRows.to_string(),
        "expected exactly one row, but the query returned none"
    );
    assert_eq!(
        ExecutionError::<&str>::MultipleRows { count: 3 }.to_string(),
        "expected at most one row, but the query returned 3"
    );
}

#[test]
fn execution_error_implements_the_error_trait() {
    fn assert_error<E: std::error::Error>() {}

    assert_error::<ExecutionError<std::io::Error>>();
}

#[test]
fn execution_error_predicates_classify_failures() {
    let backend: ExecutionError<&str> = ExecutionError::Backend("connection lost");
    assert!(!backend.is_not_found());
    assert!(!backend.is_multiple_rows());
    assert!(!backend.is_row_decode());

    let no_rows = ExecutionError::<&str>::NoRows;
    assert!(no_rows.is_not_found());
    assert!(!no_rows.is_multiple_rows());
    assert!(!no_rows.is_row_decode());

    let multiple = ExecutionError::<&str>::MultipleRows { count: 2 };
    assert!(multiple.is_multiple_rows());
    assert!(!multiple.is_not_found());
    assert!(!multiple.is_row_decode());

    let decode = ExecutionError::<&str>::RowDecode {
        row_index: 3,
        source: crate::row::RowDecodeError::TypeMismatch {
            target: "i64",
            found: "text",
        },
    };
    assert!(decode.is_row_decode());
    assert!(!decode.is_not_found());
    assert!(!decode.is_multiple_rows());
}

#[test]
fn execution_error_backend_accessors_expose_driver_errors() {
    let backend: ExecutionError<String> = ExecutionError::Backend("timeout".to_owned());

    assert_eq!(backend.as_backend(), Some(&"timeout".to_owned()));
    assert_eq!(backend.into_backend(), Some("timeout".to_owned()));

    let no_rows = ExecutionError::<String>::NoRows;
    assert_eq!(no_rows.as_backend(), None);
    assert_eq!(no_rows.into_backend(), None);

    let multiple = ExecutionError::<String>::MultipleRows { count: 2 };
    assert_eq!(multiple.as_backend(), None);
    assert_eq!(multiple.into_backend(), None);

    let decode = ExecutionError::<String>::RowDecode {
        row_index: 0,
        source: crate::row::RowDecodeError::TypeMismatch {
            target: "i64",
            found: "text",
        },
    };
    assert_eq!(decode.as_backend(), None);
    assert_eq!(
        decode.as_row_decode(),
        Some((
            0,
            &crate::row::RowDecodeError::TypeMismatch {
                target: "i64",
                found: "text",
            }
        ))
    );
    assert_eq!(decode.into_backend(), None);
}

#[test]
fn backend_errors_convert_into_execution_errors() {
    let converted: ExecutionError<&str> = "timeout".into();

    assert_eq!(converted, ExecutionError::Backend("timeout"));
    assert_eq!(converted.as_backend(), Some(&"timeout"));
    assert!(!converted.is_row_decode());
}

#[test]
fn async_helpers_decode_rows_behind_a_send_future() {
    let backend = MockAsyncBackend {
        rows: vec![
            vec![SqlVal::I64(11), SqlVal::Text("root".to_owned())],
            vec![SqlVal::I64(12), SqlVal::Text("guest".to_owned())],
        ],
        ..MockAsyncBackend::default()
    };
    let query = async_user_query();

    let rows = block_on(spawnable(backend.fetch_all(&query))).expect("rows decode");

    assert_eq!(
        rows,
        vec![(11i64, "root".to_owned()), (12i64, "guest".to_owned())]
    );
}

#[test]
fn async_single_row_helpers_report_missing_and_duplicate_rows() {
    let absent = MockAsyncBackend::default();
    assert_eq!(
        block_on(absent.fetch_one(&async_user_query())),
        Err(ExecutionError::NoRows)
    );
    assert_eq!(
        block_on(absent.fetch_optional(&async_user_query())),
        Ok(None)
    );

    let duplicated = MockAsyncBackend {
        rows: vec![
            vec![SqlVal::I64(1), SqlVal::Text("root".to_owned())],
            vec![SqlVal::I64(2), SqlVal::Text("guest".to_owned())],
        ],
        ..MockAsyncBackend::default()
    };
    assert_eq!(
        block_on(duplicated.fetch_one(&async_user_query())),
        Err(ExecutionError::MultipleRows { count: 2 })
    );

    let single = MockAsyncBackend {
        rows: vec![vec![SqlVal::I64(7), SqlVal::Text("root".to_owned())]],
        ..MockAsyncBackend::default()
    };
    assert_eq!(
        block_on(single.fetch_optional(&async_user_query())),
        Ok(Some((7i64, "root".to_owned())))
    );
}

#[test]
fn async_helpers_report_the_row_that_fails_to_decode() {
    let backend = MockAsyncBackend {
        rows: vec![vec![
            SqlVal::Text("oops".to_owned()),
            SqlVal::Text("root".to_owned()),
        ]],
        ..MockAsyncBackend::default()
    };

    assert_eq!(
        block_on(backend.fetch_all(&async_user_query())),
        Err(ExecutionError::RowDecode {
            row_index: 0,
            source: crate::row::RowDecodeError::TypeMismatch {
                target: std::any::type_name::<i64>(),
                found: "text",
            },
        })
    );
}

#[test]
fn async_execute_reports_affected_rows_without_decoding() {
    let backend = MockAsyncBackend {
        affected: 2,
        ..MockAsyncBackend::default()
    };
    let query =
        InsertQuery::<MockDialect, User>::new().values((UserId::set(1i64), UserName::set("root")));

    assert_eq!(block_on(backend.execute(&query)), Ok(2));
}

#[test]
fn async_futures_run_on_a_spawned_thread() {
    let backend = std::sync::Arc::new(MockAsyncBackend {
        rows: vec![vec![SqlVal::I64(1), SqlVal::Text("root".to_owned())]],
        ..MockAsyncBackend::default()
    });
    let worker = backend.clone();

    let rows = std::thread::spawn(move || {
        let query = async_user_query();
        block_on(worker.fetch_all(&query))
    })
    .join()
    .expect("the worker thread finishes")
    .expect("rows decode");

    assert_eq!(rows, vec![(1i64, "root".to_owned())]);
}

#[test]
fn greatest_and_least_feed_a_filter_of_the_statement() {
    // The scalar functions ride on the same expression tree as any other
    // predicate, so a comparison of their result stays scope-checked.
    let built = SelectQuery::<MockDialect>::new()
        .from::<Order>()
        .select((OrderId,))
        .where_(crate::expr::greatest(OrderTotal, [1.0]).gt(0.0))
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "SELECT \"id\" FROM \"orders\" WHERE (GREATEST(\"total\", $1) > $2)"
    );
    assert_eq!(built.params, vec![SqlVal::F64(1.0), SqlVal::F64(0.0)]);
}

#[test]
fn a_mutation_predicate_renders_against_the_mutated_table() {
    // The filter is checked against the mutated table at the call site, so a
    // column of an unrelated relation never reaches the renderer: that case is
    // rejected by the compiler, and the statement built here reads the table the
    // `UPDATE` mutates.
    let built = UpdateQuery::<MockDialect, User>::new()
        .set(UserName::set("root"))
        .where_(Expr::column::<UserId>().eq(1i64))
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "UPDATE \"users\" SET \"name\" = $1 WHERE (\"id\" = $2)"
    );
    assert_eq!(
        built.params,
        vec![SqlVal::Text("root".to_owned()), SqlVal::I64(1)]
    );
}

/// A backend recording the read limit every fetch asks for.
///
/// `honour_limit` models an adapter that stops reading the cursor once the
/// limit is reached. With `honour_limit: false` the backend instead behaves like
/// a third-party implementation that only overrides `fetch_raw`: it receives a
/// limit yet hands back the whole result set, which the execution helpers must
/// still classify correctly.
#[derive(Default)]
struct RecordingBackend {
    rows: Vec<Vec<SqlVal>>,
    honour_limit: bool,
    limits: Vec<Option<usize>>,
}

impl RecordingBackend {
    /// Logs `limit` and hands back the canned rows, truncated when the backend
    /// honours the bound.
    fn record(&mut self, limit: Option<usize>) -> Vec<Vec<SqlVal>> {
        self.limits.push(limit);
        let mut rows = self.rows.clone();
        if let (true, Some(limit)) = (self.honour_limit, limit) {
            rows.truncate(limit);
        }
        rows
    }
}

impl QueryBackend for RecordingBackend {
    type Error = &'static str;

    fn fetch_raw(&mut self, _query: &BuiltQuery) -> Result<Vec<Vec<SqlVal>>, Self::Error> {
        Ok(self.record(None))
    }

    fn fetch_raw_limited(
        &mut self,
        _query: &BuiltQuery,
        limit: Option<usize>,
    ) -> Result<Vec<Vec<SqlVal>>, Self::Error> {
        Ok(self.record(limit))
    }

    fn execute_raw(&mut self, _query: &BuiltQuery) -> Result<u64, Self::Error> {
        Ok(0)
    }
}

/// The asynchronous counterpart of [`RecordingBackend`].
#[derive(Default)]
struct AsyncRecordingBackend {
    rows: Vec<Vec<SqlVal>>,
    limits: std::sync::Mutex<Vec<Option<usize>>>,
}

impl AsyncRecordingBackend {
    /// Logs `limit` and hands back the canned rows truncated to the bound.
    fn record(&self, limit: Option<usize>) -> Vec<Vec<SqlVal>> {
        self.limits
            .lock()
            .expect("the limit log is not poisoned")
            .push(limit);
        let mut rows = self.rows.clone();
        if let Some(limit) = limit {
            rows.truncate(limit);
        }
        rows
    }

    /// The limits observed so far, in call order.
    fn limits(&self) -> Vec<Option<usize>> {
        self.limits
            .lock()
            .expect("the limit log is not poisoned")
            .clone()
    }
}

impl AsyncQueryBackend for AsyncRecordingBackend {
    type Error = &'static str;

    fn fetch_raw(
        &self,
        _query: &BuiltQuery,
    ) -> impl core::future::Future<Output = Result<Vec<Vec<SqlVal>>, Self::Error>> + Send {
        let rows = self.record(None);
        async move { Ok(rows) }
    }

    fn fetch_raw_limited(
        &self,
        _query: &BuiltQuery,
        limit: Option<usize>,
    ) -> impl core::future::Future<Output = Result<Vec<Vec<SqlVal>>, Self::Error>> + Send {
        let rows = self.record(limit);
        async move { Ok(rows) }
    }

    async fn execute_raw(&self, _query: &BuiltQuery) -> Result<u64, Self::Error> {
        Ok(0)
    }
}

fn user_id_query() -> SelectQuery<MockDialect, User, (i64,), HasProjection> {
    SelectQuery::<MockDialect>::new()
        .from::<User>()
        .select((UserId,))
}

#[test]
fn fetch_all_reads_the_unbounded_result_set() {
    let mut backend = RecordingBackend {
        rows: vec![
            vec![SqlVal::I64(1)],
            vec![SqlVal::I64(2)],
            vec![SqlVal::I64(3)],
        ],
        honour_limit: true,
        ..RecordingBackend::default()
    };

    assert_eq!(
        backend.fetch_all(&user_id_query()),
        Ok(vec![(1i64,), (2i64,), (3i64,)])
    );
    assert_eq!(backend.limits, vec![None]);
}

#[test]
fn fetch_optional_asks_for_two_rows_even_when_none_are_returned() {
    let mut backend = RecordingBackend {
        honour_limit: true,
        ..RecordingBackend::default()
    };

    assert_eq!(backend.fetch_optional(&user_id_query()), Ok(None));
    assert_eq!(backend.limits, vec![Some(2)]);
}

#[test]
fn fetch_optional_reads_two_rows_and_decodes_the_confirmed_single_row() {
    let mut backend = RecordingBackend {
        rows: vec![vec![SqlVal::I64(7)]],
        honour_limit: true,
        ..RecordingBackend::default()
    };

    assert_eq!(backend.fetch_optional(&user_id_query()), Ok(Some((7i64,))));
    assert_eq!(backend.limits, vec![Some(2)]);
}

#[test]
fn fetch_one_reads_two_rows_and_decodes_the_confirmed_single_row() {
    let mut backend = RecordingBackend {
        rows: vec![vec![SqlVal::I64(7)]],
        honour_limit: true,
        ..RecordingBackend::default()
    };

    assert_eq!(backend.fetch_one(&user_id_query()), Ok((7i64,)));
    assert_eq!(backend.limits, vec![Some(2)]);
}

#[test]
fn fetch_optional_bounds_the_read_of_a_long_result_set() {
    // The cursor is read for two rows only, so the reported cardinality is the
    // bounded count rather than the size of the full result set.
    let mut backend = RecordingBackend {
        rows: vec![
            vec![SqlVal::I64(1)],
            vec![SqlVal::I64(2)],
            vec![SqlVal::I64(3)],
        ],
        honour_limit: true,
        ..RecordingBackend::default()
    };

    assert_eq!(
        backend.fetch_optional(&user_id_query()),
        Err(ExecutionError::MultipleRows { count: 2 })
    );
    assert_eq!(backend.limits, vec![Some(2)]);
}

#[test]
fn fetch_optional_reports_multiple_rows_without_decoding_them() {
    // The second row cannot decode into `i64`, yet the cardinality violation is
    // detected first: the bounded read proves that two rows exist, and the
    // decoder never runs on either of them.
    let mut backend = RecordingBackend {
        rows: vec![
            vec![SqlVal::I64(1)],
            vec![SqlVal::Text("not-a-number".to_owned())],
        ],
        honour_limit: true,
        ..RecordingBackend::default()
    };

    let error = backend
        .fetch_optional(&user_id_query())
        .expect_err("two rows violate the single-row contract");

    assert_eq!(error, ExecutionError::MultipleRows { count: 2 });
    assert!(!error.is_row_decode());
    assert_eq!(backend.limits, vec![Some(2)]);
}

#[test]
fn fetch_one_reports_multiple_rows_without_decoding_them() {
    let mut backend = RecordingBackend {
        rows: vec![
            vec![SqlVal::Text("not-a-number".to_owned())],
            vec![SqlVal::I64(2)],
        ],
        honour_limit: true,
        ..RecordingBackend::default()
    };

    let error = backend
        .fetch_one(&user_id_query())
        .expect_err("two rows violate the single-row contract");

    assert_eq!(error, ExecutionError::MultipleRows { count: 2 });
    assert!(!error.is_row_decode());
    assert_eq!(backend.limits, vec![Some(2)]);
}

#[test]
fn a_backend_ignoring_the_limit_still_reports_multiple_rows_without_decoding() {
    // The execution helper validates cardinality on whatever the backend hands
    // back, so an adapter that never truncates cannot turn the violation into a
    // decode error either.
    let mut backend = RecordingBackend {
        rows: vec![
            vec![SqlVal::I64(1)],
            vec![SqlVal::Text("not-a-number".to_owned())],
            vec![SqlVal::I64(3)],
        ],
        honour_limit: false,
        ..RecordingBackend::default()
    };

    let error = backend
        .fetch_optional(&user_id_query())
        .expect_err("three rows violate the single-row contract");

    assert_eq!(error, ExecutionError::MultipleRows { count: 3 });
    assert!(!error.is_row_decode());
    assert_eq!(backend.limits, vec![Some(2)]);
}

#[test]
fn async_fetch_optional_asks_for_two_rows_even_when_none_are_returned() {
    let backend = AsyncRecordingBackend::default();

    assert_eq!(block_on(backend.fetch_optional(&user_id_query())), Ok(None));
    assert_eq!(backend.limits(), vec![Some(2)]);
}

#[test]
fn async_fetch_optional_reads_two_rows_and_decodes_the_confirmed_single_row() {
    let backend = AsyncRecordingBackend {
        rows: vec![vec![SqlVal::I64(7)]],
        ..AsyncRecordingBackend::default()
    };

    assert_eq!(
        block_on(backend.fetch_optional(&user_id_query())),
        Ok(Some((7i64,)))
    );
    assert_eq!(backend.limits(), vec![Some(2)]);
}

#[test]
fn async_fetch_optional_reports_multiple_rows_without_decoding_them() {
    let backend = AsyncRecordingBackend {
        rows: vec![
            vec![SqlVal::I64(1)],
            vec![SqlVal::Text("not-a-number".to_owned())],
        ],
        ..AsyncRecordingBackend::default()
    };

    let error = block_on(backend.fetch_optional(&user_id_query()))
        .expect_err("two rows violate the single-row contract");

    assert_eq!(error, ExecutionError::MultipleRows { count: 2 });
    assert!(!error.is_row_decode());
    assert_eq!(backend.limits(), vec![Some(2)]);
}

#[test]
fn async_fetch_one_reads_two_rows_and_reports_the_bound_violation() {
    let backend = AsyncRecordingBackend {
        rows: vec![
            vec![SqlVal::I64(1)],
            vec![SqlVal::I64(2)],
            vec![SqlVal::I64(3)],
        ],
        ..AsyncRecordingBackend::default()
    };

    assert_eq!(
        block_on(backend.fetch_one(&user_id_query())),
        Err(ExecutionError::MultipleRows { count: 2 })
    );
    assert_eq!(backend.limits(), vec![Some(2)]);
}

#[test]
fn async_fetch_all_reads_the_unbounded_result_set() {
    let backend = AsyncRecordingBackend {
        rows: vec![
            vec![SqlVal::I64(1)],
            vec![SqlVal::I64(2)],
            vec![SqlVal::I64(3)],
        ],
        ..AsyncRecordingBackend::default()
    };

    assert_eq!(
        block_on(backend.fetch_all(&user_id_query())),
        Ok(vec![(1i64,), (2i64,), (3i64,)])
    );
    assert_eq!(backend.limits(), vec![None]);
}

#[test]
fn fetch_collect_decodes_rows_into_a_hash_map() {
    // The collection is chosen at the call site, so a decoded pair becomes a key
    // and its value without an intermediate `Vec` of rows.
    let query = SelectQuery::<MockDialect>::new()
        .from::<User>()
        .select((UserId, UserName));
    let mut backend = MockBackend {
        rows: vec![
            vec![SqlVal::I64(1), SqlVal::Text("root".to_owned())],
            vec![SqlVal::I64(2), SqlVal::Text("guest".to_owned())],
        ],
        ..MockBackend::default()
    };

    let map = backend
        .fetch_collect::<std::collections::HashMap<i64, String>, _>(&query)
        .expect("rows decode");

    assert_eq!(map.len(), 2);
    assert_eq!(map.get(&1), Some(&"root".to_owned()));
    assert_eq!(map.get(&2), Some(&"guest".to_owned()));
}

#[test]
fn fetch_collect_decodes_rows_into_a_hash_set_and_a_btree_set() {
    let mut backend = MockBackend {
        rows: vec![
            vec![SqlVal::I64(2)],
            vec![SqlVal::I64(1)],
            vec![SqlVal::I64(2)],
        ],
        ..MockBackend::default()
    };

    let unordered = backend
        .fetch_collect::<std::collections::HashSet<(i64,)>, _>(&user_id_query())
        .expect("rows decode");
    assert_eq!(
        unordered,
        std::collections::HashSet::from([(1i64,), (2i64,)])
    );

    let ordered = backend
        .fetch_collect::<std::collections::BTreeSet<(i64,)>, _>(&user_id_query())
        .expect("rows decode");
    assert_eq!(
        ordered,
        std::collections::BTreeSet::from([(1i64,), (2i64,)])
    );
}

#[test]
fn fetch_collect_decodes_rows_into_a_deque_in_result_order() {
    let mut backend = MockBackend {
        rows: vec![
            vec![SqlVal::I64(3)],
            vec![SqlVal::I64(1)],
            vec![SqlVal::I64(2)],
        ],
        ..MockBackend::default()
    };

    let deque = backend
        .fetch_collect::<std::collections::VecDeque<(i64,)>, _>(&user_id_query())
        .expect("rows decode");

    assert_eq!(
        deque.into_iter().collect::<Vec<_>>(),
        vec![(3i64,), (1i64,), (2i64,)]
    );
}

#[test]
fn fetch_collect_reports_the_row_that_fails_to_decode() {
    let mut backend = MockBackend {
        rows: vec![vec![SqlVal::I64(1)], vec![SqlVal::Text("oops".to_owned())]],
        ..MockBackend::default()
    };

    let error = backend
        .fetch_collect::<std::collections::HashSet<(i64,)>, _>(&user_id_query())
        .expect_err("a text value does not decode as an i64");

    assert!(error.is_row_decode());
    assert_eq!(
        error.as_row_decode().map(|(row_index, _)| row_index),
        Some(1)
    );
}

#[test]
fn fetch_iter_decodes_rows_only_when_they_are_consumed() {
    let mut backend = MockBackend {
        rows: vec![
            vec![SqlVal::I64(1)],
            vec![SqlVal::Text("oops".to_owned())],
            vec![SqlVal::I64(3)],
        ],
        ..MockBackend::default()
    };

    let mut rows = backend.fetch_iter(&user_id_query()).expect("rows are read");

    // The raw buffer is read up front; decoding waits for `next`.
    assert_eq!(rows.decoded(), 0);
    assert_eq!(rows.size_hint(), (3, Some(3)));

    assert_eq!(rows.next(), Some(Ok((1i64,))));
    assert_eq!(rows.decoded(), 1);

    let error = rows
        .next()
        .expect("the second row is present")
        .expect_err("a text value does not decode as an i64");
    assert!(error.is_row_decode());
    assert_eq!(rows.decoded(), 2);

    assert_eq!(rows.next(), Some(Ok((3i64,))));
    assert_eq!(rows.next(), None);
    assert_eq!(rows.decoded(), 3);
}

#[test]
fn fetch_iter_streams_rows_without_collecting_them() {
    let mut backend = MockBackend {
        rows: vec![
            vec![SqlVal::I64(1)],
            vec![SqlVal::I64(2)],
            vec![SqlVal::I64(3)],
            vec![SqlVal::I64(4)],
        ],
        ..MockBackend::default()
    };

    let total = backend
        .fetch_iter(&user_id_query())
        .expect("rows are read")
        .map(|row| row.expect("every id decodes").0)
        .sum::<i64>();

    assert_eq!(total, 10);
}

#[test]
fn query_terminals_run_against_a_synchronous_backend() {
    // The borrow the caller passes selects the runtime: `&mut conn` runs the
    // statement synchronously and hands the decoded rows straight back.
    let mut backend = MockBackend {
        rows: vec![vec![SqlVal::I64(1)], vec![SqlVal::I64(2)]],
        affected: 5,
        ..MockBackend::default()
    };

    assert_eq!(
        user_id_query().fetch_all(&mut backend),
        Ok(vec![(1i64,), (2i64,)])
    );
    assert_eq!(
        user_id_query().fetch_one(&mut backend),
        Err(ExecutionError::MultipleRows { count: 2 })
    );

    backend.rows = vec![vec![SqlVal::I64(7)]];
    assert_eq!(
        user_id_query().fetch_optional(&mut backend),
        Ok(Some((7i64,)))
    );
    assert_eq!(user_id_query().fetch_one(&mut backend), Ok((7i64,)));

    let insert =
        InsertQuery::<MockDialect, User>::new().values((UserId::set(1i64), UserName::set("root")));
    assert_eq!(insert.execute(&mut backend), Ok(5));
}

#[test]
fn query_terminals_await_an_asynchronous_backend() {
    // The very same chain ends in an awaited terminal when the backend is
    // passed by shared reference, and the future stays movable between tasks.
    let backend = MockAsyncBackend {
        rows: vec![vec![SqlVal::I64(7)]],
        affected: 3,
    };

    assert_eq!(
        block_on(spawnable(user_id_query().fetch_all(&backend))),
        Ok(vec![(7i64,)])
    );
    assert_eq!(
        block_on(spawnable(user_id_query().fetch_optional(&backend))),
        Ok(Some((7i64,)))
    );
    assert_eq!(
        block_on(spawnable(user_id_query().fetch_one(&backend))),
        Ok((7i64,))
    );

    let insert =
        InsertQuery::<MockDialect, User>::new().values((UserId::set(1i64), UserName::set("root")));
    assert_eq!(block_on(spawnable(insert.execute(&backend))), Ok(3));
}
