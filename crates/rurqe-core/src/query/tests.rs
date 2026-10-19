use super::*;

use crate::dialect::{Dialect, SupportsLateralJoin};
use crate::dml::{ColumnAssign, DeleteQuery, Filtered, InsertQuery, UpdateQuery};
use crate::expr::{Expr, SqlVal};
use crate::row::FromRow;
use crate::schema::{Aliased, AliasedCol, At, Column, Next, Table, TableInScope};

struct MockDialect;

impl Dialect for MockDialect {
    fn push_quoted_identifier(out: &mut String, ident: &str) {
        out.push('"');
        out.push_str(ident);
        out.push('"');
    }

    fn push_placeholder(out: &mut String, index: usize) {
        use core::fmt::Write;
        let _ = write!(out, "${index}");
    }
}

impl SupportsLateralJoin for MockDialect {}

#[derive(Default)]
struct MockBackend {
    rows: Vec<Vec<SqlVal>>,
    affected: u64,
    executed: Vec<BuiltQuery>,
}

impl QueryBackend for MockBackend {
    type Error = &'static str;

    fn fetch_raw(&mut self, query: &BuiltQuery) -> Result<Vec<Vec<SqlVal>>, Self::Error> {
        self.executed.push(query.clone());
        Ok(self.rows.clone())
    }

    fn execute_raw(&mut self, query: &BuiltQuery) -> Result<u64, Self::Error> {
        self.executed.push(query.clone());
        Ok(self.affected)
    }
}

crate::table! {
    User => "users" {
        UserId => "id": i64,
        UserName => "name": String,
    }
}

crate::table! {
    Order => "orders" {
        OrderId => "id": i64,
        OrderUserId => "user_id": i64,
        OrderTotal => "total": f64,
    }
}

#[test]
fn renders_selection_from_filter_and_pagination() {
    let built = SelectQuery::<MockDialect>::new()
        .from::<User>()
        .select((UserId, UserName))
        .where_(Expr::column::<UserId>().gt(10i64))
        .limit(20)
        .offset(5)
        .build();

    assert_eq!(
        built.sql,
        "SELECT \"id\", \"name\" FROM \"users\" WHERE (\"id\" > $1) LIMIT 20 OFFSET 5"
    );
    assert_eq!(built.params, vec![SqlVal::I64(10)]);
}

#[test]
fn selection_determines_row_type() {
    let query: SelectQuery<MockDialect, User, (i64,)> =
        SelectQuery::<MockDialect>::new().from::<User>().select((UserId,));

    assert_eq!(query.build().sql, "SELECT \"id\" FROM \"users\"");
}

#[test]
fn projections_and_table_may_be_declared_in_any_order() {
    let built = SelectQuery::<MockDialect>::new()
        .select((UserId,))
        .from::<User>()
        .build();

    assert_eq!(built.sql, "SELECT \"id\" FROM \"users\"");
}

#[test]
fn repeated_predicates_are_combined_with_and() {
    let built = SelectQuery::<MockDialect>::new()
        .from::<User>()
        .select((UserId, UserName))
        .where_(Expr::column::<UserId>().gt(1i64))
        .where_(Expr::column::<UserName>().eq("root"))
        .build();

    assert_eq!(
        built.sql,
        "SELECT \"id\", \"name\" FROM \"users\" WHERE ((\"id\" > $1) AND (\"name\" = $2))"
    );
    assert_eq!(
        built.params,
        vec![SqlVal::I64(1), SqlVal::Text("root".to_owned())]
    );
}

#[test]
fn order_by_renders_asc_and_desc() {
    let built = SelectQuery::<MockDialect>::new()
        .from::<User>()
        .select((UserId, UserName))
        .order_by(UserId, OrderDirection::Desc)
        .order_by_asc(UserName)
        .build();

    assert_eq!(
        built.sql,
        "SELECT \"id\", \"name\" FROM \"users\" ORDER BY \"id\" DESC, \"name\" ASC"
    );
}

#[test]
#[should_panic(expected = "at least one projection")]
fn building_without_projections_panics() {
    let _ = SelectQuery::<MockDialect>::new().from::<User>().build();
}

#[test]
fn inner_join_renders_join_clause_before_filter() {
    let query: SelectQuery<MockDialect, Joined<User, Order>, (i64, i64)> =
        SelectQuery::<MockDialect>::new()
            .from::<User>()
            .inner_join::<Order>(Expr::column::<OrderUserId>().eq(7i64))
            .select((UserId, OrderId));

    let built = query.where_(Expr::column::<UserId>().gt(1i64)).build();

    assert_eq!(
        built.sql,
        "SELECT \"id\", \"id\" FROM \"users\" INNER JOIN \"orders\" \
         ON (\"user_id\" = $1) WHERE (\"id\" > $2)"
    );
    assert_eq!(built.params, vec![SqlVal::I64(7), SqlVal::I64(1)]);
}

#[test]
fn joined_scope_tracks_every_table_in_from_order() {
    let built = SelectQuery::<MockDialect>::new()
        .from::<User>()
        .inner_join::<Order>(Expr::column::<OrderUserId>().eq(1i64))
        .inner_join::<User>(Expr::column::<UserId>().eq(2i64))
        .select((OrderId,))
        .build();

    assert_eq!(
        built.sql,
        "SELECT \"id\" FROM \"users\" INNER JOIN \"orders\" ON (\"user_id\" = $1) \
         INNER JOIN \"users\" ON (\"id\" = $2)"
    );
    assert_eq!(built.params, vec![SqlVal::I64(1), SqlVal::I64(2)]);
}

#[test]
fn single_table_and_joined_scopes_are_query_scopes() {
    fn assert_scope<S: TableScope>() {}

    assert_scope::<User>();
    assert_scope::<Joined<User, Order>>();
    assert_scope::<LeftJoined<User, Order>>();
    assert_scope::<Joined<Joined<User, Order>, User>>();
    assert_scope::<Joined<LeftJoined<User, Order>, User>>();
}

#[test]
fn joined_scopes_resolve_table_positions_deterministically() {
    fn assert_in_scope<S, T: Table, P>()
    where
        S: TableInScope<T, P>,
    {
    }

    // Base case: the newly joined table sits at `At<0>`.
    assert_in_scope::<Joined<User, Order>, Order, At<0>>();
    assert_in_scope::<LeftJoined<User, Order>, Order, At<0>>();

    // Inductive case: inner tables sit one position deeper.
    assert_in_scope::<Joined<User, Order>, User, Next<At<0>>>();
    assert_in_scope::<LeftJoined<User, Order>, User, Next<At<0>>>();

    // Multi-join: every table keeps a distinct position marker.
    assert_in_scope::<Joined<Joined<User, Order>, User>, User, At<0>>();
    assert_in_scope::<Joined<Joined<User, Order>, User>, Order, Next<At<0>>>();
    assert_in_scope::<Joined<Joined<User, Order>, User>, User, Next<Next<At<0>>>>();
}

#[test]
fn lateral_scopes_resolve_table_positions_deterministically() {
    fn assert_in_scope<S, T: Table, P>()
    where
        S: TableInScope<T, P>,
    {
    }

    assert_in_scope::<CrossLateralJoined<User, RecentOrder>, RecentOrder, At<0>>();
    assert_in_scope::<CrossLateralJoined<User, RecentOrder>, User, Next<At<0>>>();
    assert_in_scope::<LeftLateralJoined<User, RecentOrder>, RecentOrder, At<0>>();
    assert_in_scope::<LeftLateralJoined<User, RecentOrder>, User, Next<At<0>>>();
}

#[test]
fn left_join_renders_left_keyword_and_composes_with_inner_join() {
    let query = SelectQuery::<MockDialect>::new()
        .from::<User>()
        .left_join::<Order>(Expr::column::<OrderUserId>().eq(7i64))
        .inner_join::<User>(Expr::column::<UserId>().eq(2i64));
    let total = query.nullable::<OrderTotal>();

    let built = query.select((OrderId, total)).build();

    assert_eq!(
        built.sql,
        "SELECT \"id\", \"total\" FROM \"users\" LEFT JOIN \"orders\" ON (\"user_id\" = $1) \
         INNER JOIN \"users\" ON (\"id\" = $2)"
    );
    assert_eq!(built.params, vec![SqlVal::I64(7), SqlVal::I64(2)]);
}

#[test]
fn nullable_column_keeps_name_and_lifts_rust_type() {
    fn takes_optional_total<C>()
    where
        C: Column<RustType = Option<f64>, Table = Order>,
    {
    }

    assert_eq!(Nullable::<OrderTotal>::NAME, OrderTotal::NAME);
    takes_optional_total::<Nullable<OrderTotal>>();
}

#[test]
fn left_joined_columns_are_lifted_without_explicit_markers() {
    let query: SelectQuery<MockDialect, LeftJoined<User, Order>, (i64, Option<f64>)> =
        SelectQuery::<MockDialect>::new()
            .from::<User>()
            .left_join::<Order>(Expr::column::<OrderUserId>().eq(7i64))
            .select((UserId, OrderTotal));

    let built = query.build();

    assert_eq!(
        built.sql,
        "SELECT \"id\", \"total\" FROM \"users\" LEFT JOIN \"orders\" ON (\"user_id\" = $1)"
    );
    assert_eq!(built.params, vec![SqlVal::I64(7)]);
}

#[test]
fn inner_joined_columns_keep_their_declared_types() {
    let query: SelectQuery<MockDialect, Joined<User, Order>, (i64, i64, f64)> =
        SelectQuery::<MockDialect>::new()
            .from::<User>()
            .inner_join::<Order>(Expr::column::<UserId>().eq(Expr::column::<OrderUserId>()))
            .select((UserId, OrderId, OrderTotal));

    assert_eq!(
        query.build().sql,
        "SELECT \"id\", \"id\", \"total\" FROM \"users\" \
         INNER JOIN \"orders\" ON (\"id\" = \"user_id\")"
    );
}

#[test]
fn nested_left_join_stays_nullable_inside_an_inner_join() {
    type NestedScope = Joined<LeftJoined<User, Order>, User>;

    let query: SelectQuery<MockDialect, NestedScope, (Option<i64>,)> =
        SelectQuery::<MockDialect>::new()
            .from::<User>()
            .left_join::<Order>(Expr::column::<OrderUserId>().eq(1i64))
            .inner_join::<User>(Expr::column::<UserId>().eq(2i64))
            .select((OrderId,));

    assert_eq!(
        query.build().sql,
        "SELECT \"id\" FROM \"users\" LEFT JOIN \"orders\" ON (\"user_id\" = $1) \
         INNER JOIN \"users\" ON (\"id\" = $2)"
    );
}

#[test]
fn explicit_nullable_marker_matches_automatic_lifting() {
    fn nullable_scope() -> SelectQuery<MockDialect, LeftJoined<User, Order>, (i64, Option<f64>)> {
        SelectQuery::<MockDialect>::new()
            .from::<User>()
            .left_join::<Order>(Expr::column::<OrderUserId>().eq(3i64))
            .select((UserId, OrderTotal))
    }

    let automatic = nullable_scope();
    let explicit = {
        let query = SelectQuery::<MockDialect>::new()
            .from::<User>()
            .left_join::<Order>(Expr::column::<OrderUserId>().eq(3i64));
        let total = query.nullable::<OrderTotal>();
        let query: SelectQuery<MockDialect, LeftJoined<User, Order>, (i64, Option<f64>)> =
            query.select((UserId, total));
        query
    };

    assert_eq!(automatic.build(), explicit.build());

    // An already nullable column keeps a single layer of `Option`.
    let single_layer: SelectQuery<MockDialect, LeftJoined<User, Order>, (Option<f64>,)> =
        SelectQuery::<MockDialect>::new()
            .from::<User>()
            .left_join::<Order>(Expr::column::<OrderUserId>().eq(3i64))
            .select((Nullable::<OrderTotal>::new(),));

    assert_eq!(
        single_layer.build().sql,
        "SELECT \"total\" FROM \"users\" LEFT JOIN \"orders\" ON (\"user_id\" = $1)"
    );
}

#[test]
fn group_by_renders_single_column_and_tuple() {
    let single = SelectQuery::<MockDialect>::new()
        .from::<User>()
        .select((UserId,))
        .group_by(UserId)
        .build();
    assert_eq!(single.sql, "SELECT \"id\" FROM \"users\" GROUP BY \"id\"");

    let tuple = SelectQuery::<MockDialect>::new()
        .from::<User>()
        .select((UserId, UserName))
        .group_by((UserId, UserName))
        .build();
    assert_eq!(
        tuple.sql,
        "SELECT \"id\", \"name\" FROM \"users\" GROUP BY \"id\", \"name\""
    );
}

#[test]
fn multiple_group_by_calls_append_columns() {
    let built = SelectQuery::<MockDialect>::new()
        .from::<User>()
        .select((UserId, UserName))
        .group_by(UserId)
        .group_by(UserName)
        .build();

    assert_eq!(
        built.sql,
        "SELECT \"id\", \"name\" FROM \"users\" GROUP BY \"id\", \"name\""
    );
}

#[test]
fn having_renders_aggregate_filters_and_binds_parameters() {
    let built = SelectQuery::<MockDialect>::new()
        .from::<Order>()
        .select((OrderUserId,))
        .group_by(OrderUserId)
        .having(Expr::count_all().gt(5i64))
        .build();

    assert_eq!(
        built.sql,
        "SELECT \"user_id\" FROM \"orders\" GROUP BY \"user_id\" HAVING (COUNT(*) > $1)"
    );
    assert_eq!(built.params, vec![SqlVal::I64(5)]);
}

#[test]
fn repeated_having_calls_are_combined_with_and() {
    let built = SelectQuery::<MockDialect>::new()
        .from::<Order>()
        .select((OrderUserId,))
        .where_(Expr::column::<OrderId>().gt(0i64))
        .group_by(OrderUserId)
        .having(Expr::count_all().gt(2i64))
        .having(Expr::sum::<OrderTotal>().lt(100.0))
        .order_by_asc(OrderUserId)
        .build();

    assert_eq!(
        built.sql,
        "SELECT \"user_id\" FROM \"orders\" WHERE (\"id\" > $1) GROUP BY \"user_id\" \
         HAVING ((COUNT(*) > $2) AND (SUM(\"total\") < $3)) ORDER BY \"user_id\" ASC"
    );
    assert_eq!(
        built.params,
        vec![SqlVal::I64(0), SqlVal::I64(2), SqlVal::F64(100.0)]
    );
}

#[test]
fn left_joined_rows_decode_missing_columns_as_none() {
    assert_eq!(
        <(i64, Option<f64>)>::from_row(&[SqlVal::I64(1), SqlVal::Null]),
        Some((1, None))
    );
}

#[test]
fn fetch_all_renders_the_query_and_decodes_rows() {
    let query = SelectQuery::<MockDialect>::new()
        .from::<User>()
        .select((UserId, UserName))
        .where_(Expr::column::<UserId>().gt(10i64));

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
    assert_eq!(backend.executed, vec![query.build()]);
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
        Err(ExecutionError::RowDecode { row_index: 0 })
    );
}

#[test]
fn execute_reports_affected_rows_without_decoding() {
    let query = InsertQuery::<MockDialect, User>::new()
        .values((UserId::set(1i64), UserName::set("root")));

    let mut backend = MockBackend {
        affected: 1,
        ..MockBackend::default()
    };

    assert_eq!(backend.execute(&query), Ok(1));
    assert_eq!(backend.executed, vec![query.build_query()]);
}

#[test]
fn executable_queries_expose_built_sql_and_row_type() {
    let insert = InsertQuery::<MockDialect, User>::new()
        .values((UserId::set(1i64), UserName::set("root")));
    assert_eq!(insert.build_query(), insert.build());

    fn assert_row<Q: ExecutableQuery<Row = (i64, String)>>() {}

    assert_row::<SelectQuery<MockDialect, User, (i64, String)>>();
    assert_row::<InsertQuery<MockDialect, User, (), (i64, String)>>();
    assert_row::<UpdateQuery<MockDialect, User, Filtered, (i64, String)>>();
    assert_row::<DeleteQuery<MockDialect, User, Filtered, (i64, String)>>();
}

#[test]
fn execution_errors_render_actionable_messages() {
    assert_eq!(
        ExecutionError::<&str>::Backend("connection lost").to_string(),
        "database backend error: connection lost"
    );
    assert_eq!(
        ExecutionError::<&str>::RowDecode { row_index: 2 }.to_string(),
        "failed to decode row 2: values do not match the query row type"
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

    let decode = ExecutionError::<&str>::RowDecode { row_index: 3 };
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

    let decode = ExecutionError::<String>::RowDecode { row_index: 0 };
    assert_eq!(decode.as_backend(), None);
    assert_eq!(decode.into_backend(), None);
}

#[test]
fn backend_errors_convert_into_execution_errors() {
    let converted: ExecutionError<&str> = "timeout".into();

    assert_eq!(converted, ExecutionError::Backend("timeout"));
    assert_eq!(converted.as_backend(), Some(&"timeout"));
    assert!(!converted.is_row_decode());
}

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
    backend.savepoint("rurqe_sp_1").expect("savepoint");
    backend.release_savepoint("rurqe_sp_1").expect("release");
    backend
        .rollback_to_savepoint("rurqe_sp_2")
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

#[test]
fn window_functions_render_expected_over_clauses() {
    let window = Window::partition_by(UserId).order_by_asc(UserName);

    let built = SelectQuery::<MockDialect>::new()
        .from::<User>()
        .select((
            UserId,
            row_number().over(window.clone()),
            rank().over(window.clone()),
            dense_rank().over(window.clone()),
            lag(UserName).offset(2).over(Window::order_by_desc(UserId)),
            lead(UserName).over(Window::empty()),
            Expr::count_all().over(Window::partition_by(UserId)),
        ))
        .build();

    assert_eq!(
        built.sql,
        "SELECT \"id\", ROW_NUMBER() OVER (PARTITION BY \"id\" ORDER BY \"name\" ASC), \
         RANK() OVER (PARTITION BY \"id\" ORDER BY \"name\" ASC), \
         DENSE_RANK() OVER (PARTITION BY \"id\" ORDER BY \"name\" ASC), \
         LAG(\"name\", 2) OVER (ORDER BY \"id\" DESC), \
         LEAD(\"name\") OVER (), \
         COUNT(*) OVER (PARTITION BY \"id\") \
         FROM \"users\""
    );
}

#[test]
fn single_cte_renders_with_clause_and_binds_sequential_parameters() {
    let active = SelectQuery::<MockDialect>::new()
        .from::<User>()
        .select((UserId, UserName))
        .where_(Expr::column::<UserId>().gt(10i64));
    let active_cte = cte::<User, _, _, _>(active);

    let built = SelectQuery::<MockDialect>::new()
        .with(active_cte)
        .from::<User>()
        .select((UserId, UserName))
        .where_(Expr::column::<UserName>().eq("root"))
        .build();

    assert_eq!(
        built.sql,
        "WITH \"users\" AS (SELECT \"id\", \"name\" FROM \"users\" WHERE (\"id\" > $1)) \
         SELECT \"id\", \"name\" FROM \"users\" WHERE (\"name\" = $2)"
    );
    assert_eq!(
        built.params,
        vec![SqlVal::I64(10), SqlVal::Text("root".to_owned())]
    );
}

#[test]
fn multiple_ctes_render_comma_separated_clauses() {
    let u_sub = SelectQuery::<MockDialect>::new()
        .from::<User>()
        .select((UserId,))
        .where_(Expr::column::<UserId>().gt(1i64));
    let o_sub = SelectQuery::<MockDialect>::new()
        .from::<Order>()
        .select((OrderId,))
        .where_(Expr::column::<OrderTotal>().gt(100.0));

    let built = SelectQuery::<MockDialect>::new()
        .with(cte_named("active_users", u_sub))
        .with(cte_named("large_orders", o_sub))
        .from::<User>()
        .select((UserId,))
        .where_(Expr::column::<UserId>().eq(7i64))
        .build();

    assert_eq!(
        built.sql,
        "WITH \"active_users\" AS (SELECT \"id\" FROM \"users\" WHERE (\"id\" > $1)), \
         \"large_orders\" AS (SELECT \"id\" FROM \"orders\" WHERE (\"total\" > $2)) \
         SELECT \"id\" FROM \"users\" WHERE (\"id\" = $3)"
    );
    assert_eq!(
        built.params,
        vec![SqlVal::I64(1), SqlVal::F64(100.0), SqlVal::I64(7)]
    );
}

#[test]
fn cte_binds_to_table_marker_and_participates_in_joins() {
    crate::table! {
        ActiveSummary => "active_summary" {
            SummaryUserId => "user_id": i64,
            SummaryTotal => "total": f64,
        }
    }

    let summary_query = SelectQuery::<MockDialect>::new()
        .from::<Order>()
        .select((OrderUserId, OrderTotal))
        .where_(Expr::column::<OrderTotal>().gt(50.0));

    let summary_cte = summary_query.as_cte::<ActiveSummary>();

    let built = SelectQuery::<MockDialect>::new()
        .with(summary_cte)
        .from::<User>()
        .inner_join::<ActiveSummary>(Expr::column::<UserId>().eq(Expr::column::<SummaryUserId>()))
        .select((UserId, UserName, SummaryTotal))
        .where_(Expr::column::<SummaryTotal>().lt(500.0))
        .build();

    assert_eq!(
        built.sql,
        "WITH \"active_summary\" AS (SELECT \"user_id\", \"total\" FROM \"orders\" WHERE (\"total\" > $1)) \
         SELECT \"id\", \"name\", \"total\" FROM \"users\" \
         INNER JOIN \"active_summary\" ON (\"id\" = \"user_id\") \
         WHERE (\"total\" < $2)"
    );
    assert_eq!(built.params, vec![SqlVal::F64(50.0), SqlVal::F64(500.0)]);
}

#[test]
fn raw_sql_renders_sequential_placeholders_in_query_clauses() {
    let raw_pred: Expr<bool> =
        Expr::raw("strftime('%Y-%m', date) = ?", vec![SqlVal::Text("2024-01".into())]);

    let built = SelectQuery::<MockDialect>::new()
        .from::<User>()
        .select((UserId, Expr::<i64>::raw("COUNT(*)", vec![])))
        .where_(Expr::column::<UserId>().gt(10i64))
        .where_(raw_pred)
        .build();

    assert_eq!(
        built.sql,
        "SELECT \"id\", COUNT(*) FROM \"users\" WHERE ((\"id\" > $1) AND strftime('%Y-%m', date) = $2)"
    );
    assert_eq!(
        built.params,
        vec![SqlVal::I64(10), SqlVal::Text("2024-01".into())]
    );
}

#[test]
fn subquery_predicates_render_nested_queries_with_sequential_parameters() {
    let large_orders = SelectQuery::<MockDialect>::new()
        .from::<Order>()
        .select((OrderUserId,))
        .where_(Expr::column::<OrderTotal>().gt(100.0));

    let recent_orders = SelectQuery::<MockDialect>::new()
        .from::<Order>()
        .select((OrderId,))
        .where_(Expr::column::<OrderTotal>().lt(10.0));

    let built = SelectQuery::<MockDialect>::new()
        .from::<User>()
        .select((UserId,))
        .where_(Expr::column::<UserId>().in_subquery(large_orders))
        .where_(Expr::not_exists(recent_orders))
        .build();

    assert_eq!(
        built.sql,
        "SELECT \"id\" FROM \"users\" WHERE ((\"id\" IN (SELECT \"user_id\" FROM \"orders\" \
         WHERE (\"total\" > $1))) AND (NOT EXISTS (SELECT \"id\" FROM \"orders\" WHERE \
         (\"total\" < $2))))"
    );
    assert_eq!(built.params, vec![SqlVal::F64(100.0), SqlVal::F64(10.0)]);
}

#[test]
fn not_in_subquery_renders_negated_membership() {
    let totals = SelectQuery::<MockDialect>::new()
        .from::<Order>()
        .select((OrderTotal,));

    let built = SelectQuery::<MockDialect>::new()
        .from::<Order>()
        .select((OrderId,))
        .where_(Expr::column::<OrderTotal>().not_in_subquery(totals))
        .build();

    assert_eq!(
        built.sql,
        "SELECT \"id\" FROM \"orders\" WHERE (\"total\" NOT IN (SELECT \"total\" FROM \"orders\"))"
    );
    assert!(built.params.is_empty());
}

#[test]
fn union_renders_combined_query_and_binds_parameters_in_order() {
    let other = SelectQuery::<MockDialect>::new()
        .from::<User>()
        .select((UserId, UserName))
        .where_(Expr::column::<UserId>().gt(10i64));

    let built = SelectQuery::<MockDialect>::new()
        .from::<User>()
        .select((UserId, UserName))
        .where_(Expr::column::<UserId>().lt(5i64))
        .union(other)
        .build();

    assert_eq!(
        built.sql,
        "SELECT \"id\", \"name\" FROM \"users\" WHERE (\"id\" < $1) \
         UNION SELECT \"id\", \"name\" FROM \"users\" WHERE (\"id\" > $2)"
    );
    assert_eq!(built.params, vec![SqlVal::I64(5), SqlVal::I64(10)]);
}

#[test]
fn set_operation_keywords_render_expected_sql() {
    fn base() -> SelectQuery<MockDialect, User, (i64,)> {
        SelectQuery::<MockDialect>::new()
            .from::<User>()
            .select((UserId,))
    }

    assert_eq!(
        base().union(base()).build().sql,
        "SELECT \"id\" FROM \"users\" UNION SELECT \"id\" FROM \"users\""
    );
    assert_eq!(
        base().union_all(base()).build().sql,
        "SELECT \"id\" FROM \"users\" UNION ALL SELECT \"id\" FROM \"users\""
    );
    assert_eq!(
        base().intersect(base()).build().sql,
        "SELECT \"id\" FROM \"users\" INTERSECT SELECT \"id\" FROM \"users\""
    );
    assert_eq!(
        base().except(base()).build().sql,
        "SELECT \"id\" FROM \"users\" EXCEPT SELECT \"id\" FROM \"users\""
    );
}

#[test]
fn chained_set_operations_keep_trailing_order_by_and_limit_on_the_union() {
    fn base() -> SelectQuery<MockDialect, User, (i64,)> {
        SelectQuery::<MockDialect>::new()
            .from::<User>()
            .select((UserId,))
    }

    let built = base()
        .union(base())
        .union_all(base())
        .order_by_asc(UserId)
        .limit(3)
        .build();

    assert_eq!(
        built.sql,
        "SELECT \"id\" FROM \"users\" UNION SELECT \"id\" FROM \"users\" \
         UNION ALL SELECT \"id\" FROM \"users\" ORDER BY \"id\" ASC LIMIT 3"
    );
    assert!(built.params.is_empty());
}

#[test]
fn union_query_is_executable_and_decodes_rows() {
    let query = SelectQuery::<MockDialect>::new()
        .from::<User>()
        .select((UserId,))
        .union(
            SelectQuery::<MockDialect>::new()
                .from::<User>()
                .select((UserId,)),
        );

    let mut backend = MockBackend {
        rows: vec![vec![SqlVal::I64(1)]],
        ..MockBackend::default()
    };

    let rows = backend.fetch_all(&query).expect("rows decode");
    assert_eq!(rows, vec![(1i64,)]);
    assert_eq!(backend.executed, vec![query.build()]);
}

crate::table_alias! {
    /// The `u` alias.
    UserAlias => "u"
}

crate::table_alias! {
    /// The `o` alias.
    OrderAlias => "o"
}

#[test]
fn aliased_table_renders_as_clause_and_qualifies_columns() {
    let built = SelectQuery::<MockDialect>::new()
        .from::<Aliased<User, UserAlias>>()
        .select((
            AliasedCol::<UserAlias, UserId>::new(),
            AliasedCol::<UserAlias, UserName>::new(),
        ))
        .where_(Expr::column::<AliasedCol<UserAlias, UserId>>().gt(1i64))
        .build();

    assert_eq!(
        built.sql,
        "SELECT \"u\".\"id\", \"u\".\"name\" FROM \"users\" AS \"u\" WHERE (\"u\".\"id\" > $1)"
    );
    assert_eq!(built.params, vec![SqlVal::I64(1)]);
}

#[test]
fn aliased_table_participates_in_joins() {
    let built = SelectQuery::<MockDialect>::new()
        .from::<User>()
        .inner_join::<Aliased<Order, OrderAlias>>(
            Expr::column::<OrderUserId>().eq(Expr::column::<AliasedCol<OrderAlias, OrderUserId>>()),
        )
        .select((UserId, AliasedCol::<OrderAlias, OrderTotal>::new()))
        .build();

    assert_eq!(
        built.sql,
        "SELECT \"id\", \"o\".\"total\" FROM \"users\" \
         INNER JOIN \"orders\" AS \"o\" ON (\"user_id\" = \"o\".\"user_id\")"
    );
    assert!(built.params.is_empty());
}

#[test]
fn left_joined_aliased_table_lifts_columns_into_options() {
    let query = SelectQuery::<MockDialect>::new()
        .from::<User>()
        .left_join::<Aliased<Order, OrderAlias>>(
            Expr::column::<OrderUserId>().eq(Expr::column::<UserId>()),
        );
    let total = query.nullable::<AliasedCol<OrderAlias, OrderTotal>>();

    let built = query.select((UserId, total)).build();

    assert_eq!(
        built.sql,
        "SELECT \"id\", \"o\".\"total\" FROM \"users\" \
         LEFT JOIN \"orders\" AS \"o\" ON (\"user_id\" = \"id\")"
    );
}

crate::table! {
    RecentOrder => "recent" {
        RecentTotal => "total": f64,
    }
}

fn latest_order_per_user_scope() -> SelectQuery<MockDialect, Order, (f64,)> {
    SelectQuery::<MockDialect>::new()
        .from::<Order>()
        .select((OrderTotal,))
        .where_(Expr::column::<OrderUserId>().eq(Expr::column::<UserId>()))
        .limit(1)
}

#[test]
fn cross_join_lateral_renders_inline_correlated_subquery() {
    let built = SelectQuery::<MockDialect>::new()
        .from::<User>()
        .cross_join_lateral(latest_order_per_user_scope().as_lateral::<RecentOrder>())
        .select((UserId, RecentTotal))
        .build();

    assert_eq!(
        built.sql,
        "SELECT \"id\", \"total\" FROM \"users\" CROSS JOIN LATERAL \
         (SELECT \"total\" FROM \"orders\" WHERE (\"user_id\" = \"id\") LIMIT 1) AS \"recent\""
    );
    assert!(built.params.is_empty());
}

#[test]
fn left_join_lateral_renders_on_true_and_lifts_columns_into_options() {
    let query = SelectQuery::<MockDialect>::new()
        .from::<User>()
        .left_join_lateral(latest_order_per_user_scope().as_lateral::<RecentOrder>());
    let total = query.nullable::<RecentTotal>();

    let built = query.select((UserId, total)).build();

    assert_eq!(
        built.sql,
        "SELECT \"id\", \"total\" FROM \"users\" LEFT JOIN LATERAL \
         (SELECT \"total\" FROM \"orders\" WHERE (\"user_id\" = \"id\") LIMIT 1) AS \"recent\" ON true"
    );
}

#[test]
fn lateral_join_binds_subquery_parameters_before_the_outer_filter() {
    let sub = SelectQuery::<MockDialect>::new()
        .from::<Order>()
        .select((OrderTotal,))
        .where_(Expr::column::<OrderUserId>().eq(Expr::column::<UserId>()))
        .where_(Expr::column::<OrderTotal>().gt(10.0))
        .limit(1);

    let built = SelectQuery::<MockDialect>::new()
        .from::<User>()
        .cross_join_lateral(sub.as_lateral::<RecentOrder>())
        .select((UserId, RecentTotal))
        .where_(Expr::column::<UserId>().gt(1i64))
        .build();

    assert_eq!(
        built.sql,
        "SELECT \"id\", \"total\" FROM \"users\" CROSS JOIN LATERAL \
         (SELECT \"total\" FROM \"orders\" WHERE ((\"user_id\" = \"id\") AND (\"total\" > $1)) \
         LIMIT 1) AS \"recent\" WHERE (\"id\" > $2)"
    );
    assert_eq!(built.params, vec![SqlVal::F64(10.0), SqlVal::I64(1)]);
}

#[test]
fn group_by_transitions_to_the_grouped_typestate() {
    let query: GroupedQuery<MockDialect, Order, (i64,), GroupSet<EmptyGroupSet, OrderUserId>> =
        SelectQuery::<MockDialect>::new()
            .from::<Order>()
            .select((OrderUserId,))
            .group_by(OrderUserId);

    assert_eq!(
        query.build().sql,
        "SELECT \"user_id\" FROM \"orders\" GROUP BY \"user_id\""
    );
}

#[test]
fn grouped_selection_accepts_grouped_columns_and_aggregates() {
    let built = SelectQuery::<MockDialect>::new()
        .from::<Order>()
        .group_by(OrderUserId)
        .select((
            OrderUserId,
            Expr::count_all(),
            Expr::sum::<OrderTotal>(),
            Expr::avg::<OrderTotal>(),
            Expr::min::<OrderId>(),
            Expr::max::<OrderTotal>(),
        ))
        .having(Expr::count_all().gt(1i64))
        .order_by_asc(OrderUserId)
        .limit(5)
        .offset(2)
        .build();

    assert_eq!(
        built.sql,
        "SELECT \"user_id\", COUNT(*), SUM(\"total\"), AVG(\"total\"), MIN(\"id\"), \
         MAX(\"total\") FROM \"orders\" GROUP BY \"user_id\" HAVING (COUNT(*) > $1) \
         ORDER BY \"user_id\" ASC LIMIT 5 OFFSET 2"
    );
    assert_eq!(built.params, vec![SqlVal::I64(1)]);
}

#[test]
fn chained_group_by_extends_the_grouped_columns() {
    let built = SelectQuery::<MockDialect>::new()
        .from::<User>()
        .group_by(UserId)
        .group_by(UserName)
        .select((UserId, UserName))
        .build();

    assert_eq!(
        built.sql,
        "SELECT \"id\", \"name\" FROM \"users\" GROUP BY \"id\", \"name\""
    );
}

#[test]
fn grouped_selection_accepts_window_expressions() {
    let built = SelectQuery::<MockDialect>::new()
        .from::<Order>()
        .group_by(OrderUserId)
        .select((OrderUserId, row_number().over(Window::order_by_asc(OrderId))))
        .build();

    assert_eq!(
        built.sql,
        "SELECT \"user_id\", ROW_NUMBER() OVER (ORDER BY \"id\" ASC) FROM \"orders\" \
         GROUP BY \"user_id\""
    );
}

#[test]
fn grouped_queries_are_executable() {
    let query = SelectQuery::<MockDialect>::new()
        .from::<Order>()
        .group_by(OrderUserId)
        .select((OrderUserId, Expr::count_all()));

    let mut backend = MockBackend {
        rows: vec![vec![SqlVal::I64(1), SqlVal::I64(3)]],
        ..MockBackend::default()
    };

    let rows = backend.fetch_all(&query).expect("grouped rows decode");

    assert_eq!(rows, vec![(1i64, 3i64)]);
    assert_eq!(backend.executed, vec![query.build()]);

    let built = query.build();
    assert_eq!(query.into_select().build(), built);
}

#[test]
fn group_set_resolves_grouped_column_positions() {
    type Grouping = GroupSet<GroupSet<EmptyGroupSet, UserId>, UserName>;

    fn assert_grouped<G, C: Column, P>()
    where
        G: ColumnInGroupBy<C, P>,
    {
    }

    fn assert_grouped_selection<S, G, P>()
    where
        S: GroupedSelection<G, P>,
    {
    }

    assert_grouped::<Grouping, UserName, At<0>>();
    assert_grouped::<Grouping, UserId, Next<At<0>>>();
    assert_grouped_selection::<(UserId, UserName, Expr<i64>), Grouping, _>();
}

#[test]
fn value_table_argument_infers_the_from_table() {
    let built = SelectQuery::<MockDialect>::new()
        .from_table(User)
        .select((UserId, UserName))
        .where_(Expr::column::<UserId>().gt(10i64))
        .build();

    assert_eq!(
        built.sql,
        "SELECT \"id\", \"name\" FROM \"users\" WHERE (\"id\" > $1)"
    );
    assert_eq!(built.params, vec![SqlVal::I64(10)]);
}

#[test]
fn value_join_arguments_infer_joined_tables() {
    let built = SelectQuery::<MockDialect>::new()
        .from_table(User)
        .inner_join_table(Order, Expr::column::<OrderUserId>().eq(7i64))
        .select((UserId, OrderTotal))
        .build();

    assert_eq!(
        built.sql,
        "SELECT \"id\", \"total\" FROM \"users\" INNER JOIN \"orders\" ON (\"user_id\" = $1)"
    );
    assert_eq!(built.params, vec![SqlVal::I64(7)]);
}

#[test]
fn value_left_join_arguments_lift_columns_into_options() {
    let query: SelectQuery<MockDialect, LeftJoined<User, Order>, (i64, Option<f64>)> =
        SelectQuery::<MockDialect>::new()
            .from_table(User)
            .left_join_table(Order, Expr::column::<OrderUserId>().eq(7i64))
            .select((UserId, OrderTotal));

    assert_eq!(
        query.build().sql,
        "SELECT \"id\", \"total\" FROM \"users\" LEFT JOIN \"orders\" ON (\"user_id\" = $1)"
    );
}
