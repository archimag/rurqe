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
    assert_eq!(insert.build_query(), Ok(insert.build()));

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
        .build();

    assert_eq!(
        built.sql,
        "UPDATE \"users\" SET \"name\" = $1 WHERE (\"id\" = $2)"
    );
    assert_eq!(
        built.params,
        vec![SqlVal::Text("root".to_owned()), SqlVal::I64(1)]
    );
}
