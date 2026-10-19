//! End-to-end verification of relational queries through the facade crate.
//!
//! The tests drive `SelectQuery` from `rurqe::prelude` across several tables:
//! `INNER JOIN` and `LEFT JOIN` render deterministic PostgreSQL SQL, columns of
//! outer-joined tables lift to `Option<T>` at compile time, joined rows decode
//! through `FromRow`, and aggregate expressions compose with `GROUP BY` and
//! `HAVING`.

use rurqe::prelude::*;

table! {
    /// The `users` table.
    User => "users" {
        UserId => "id": i64,
        UserName => "name": String,
        UserActive => "active": bool,
    }
}

table! {
    /// The `orders` table.
    Order => "orders" {
        OrderId => "id": i64,
        OrderUserId => "user_id": i64,
        OrderTotal => "total": f64,
    }
}

table! {
    /// Summary CTE table representation.
    UserSummary => "user_summary" {
        SummaryUserId => "user_id": i64,
        SummaryOrderCount => "order_count": i64,
    }
}

/// Decodes `values` into the row type of the anonymous selection `S`.
fn decode<S>(_selection: &S, values: &[SqlVal]) -> Option<S::Row>
where
    S: Selection,
    S::Row: FromRow,
{
    S::Row::from_row(values)
}

/// Statically asserts that `S` is a valid query scope.
fn assert_scope<S: TableScope>() {}

/// Statically asserts that the selection `S` produces `(i64, Option<f64>)` rows.
fn assert_nullable_row<S: Selection<Row = (i64, Option<f64>)>>() {}

#[test]
fn inner_join_spans_multiple_tables_and_decodes_rows() {
    assert_scope::<User>();
    assert_scope::<Joined<User, Order>>();
    assert_scope::<LeftJoined<User, Order>>();

    let selection = cols!(UserId, OrderId, OrderTotal);

    let built = select_from::<Postgres<Pg16>, User>()
        .inner_join::<Order>(Expr::column::<OrderUserId>().eq(7i64))
        .select(cols!(UserId, OrderId, OrderTotal))
        .where_(Expr::column::<UserActive>().eq(true))
        .build();

    assert_eq!(
        built.sql,
        "SELECT \"users\".\"id\", \"orders\".\"id\", \"orders\".\"total\" FROM \"users\" \
         INNER JOIN \"orders\" ON (\"user_id\" = $1) WHERE (\"active\" = $2)"
    );
    assert_eq!(built.params, vec![SqlVal::I64(7), SqlVal::Bool(true)]);

    let record = decode(
        &selection,
        &[SqlVal::I64(1), SqlVal::I64(42), SqlVal::F64(19.5)],
    )
    .expect("row decodes");

    assert_eq!(record.user_id, 1);
    assert_eq!(record.order_id, 42);
    assert_eq!(record.order_total, 19.5);
}

#[test]
fn left_join_lifts_joined_columns_into_options() {
    assert_nullable_row::<(UserId, Nullable<OrderTotal>)>();

    let query = SelectQuery::<Postgres<Pg14>>::new()
        .from::<User>()
        .left_join::<Order>(Expr::column::<OrderUserId>().eq(3i64));
    let total = query.nullable::<OrderTotal>();
    let built = query.select((UserId, total)).build();

    assert_eq!(
        built.sql,
        "SELECT \"id\", \"total\" FROM \"users\" LEFT JOIN \"orders\" ON (\"user_id\" = $1)"
    );
    assert_eq!(built.params, vec![SqlVal::I64(3)]);

    let missing = <(i64, Option<f64>)>::from_row(&[SqlVal::I64(1), SqlVal::Null])
        .expect("unmatched rows decode");
    assert_eq!(missing, (1, None));

    let present = <(i64, Option<f64>)>::from_row(&[SqlVal::I64(2), SqlVal::F64(19.5)])
        .expect("matched rows decode");
    assert_eq!(present, (2, Some(19.5)));
}

#[test]
fn aggregates_compose_with_group_by_and_having() {
    let built = SelectQuery::<Postgres<Pg15>>::new()
        .from::<Order>()
        .select((OrderUserId,))
        .where_(Expr::column::<OrderId>().gt(0i64))
        .group_by(OrderUserId)
        .having(Expr::count_all().gt(2i64))
        .having(Expr::sum::<OrderTotal>().lt(500.0))
        .order_by_desc(OrderUserId)
        .limit(10)
        .build();

    assert_eq!(
        built.sql,
        "SELECT \"user_id\" FROM \"orders\" WHERE (\"id\" > $1) GROUP BY \"user_id\" \
         HAVING ((COUNT(*) > $2) AND (SUM(\"total\") < $3)) ORDER BY \"user_id\" DESC LIMIT 10"
    );
    assert_eq!(
        built.params,
        vec![SqlVal::I64(0), SqlVal::I64(2), SqlVal::F64(500.0)]
    );
}

#[test]
fn aggregate_functions_cover_avg_min_and_max() {
    let built = SelectQuery::<Postgres<Pg16>>::new()
        .from::<Order>()
        .select((OrderUserId,))
        .group_by(OrderUserId)
        .having(Expr::avg::<OrderTotal>().gt(10.0))
        .having(Expr::min::<OrderId>().lt(100i64))
        .having(Expr::max::<OrderTotal>().lt(1000.0))
        .build();

    assert_eq!(
        built.sql,
        "SELECT \"user_id\" FROM \"orders\" GROUP BY \"user_id\" HAVING \
         (((AVG(\"total\") > $1) AND (MIN(\"id\") < $2)) AND (MAX(\"total\") < $3))"
    );
    assert_eq!(
        built.params,
        vec![SqlVal::F64(10.0), SqlVal::I64(100), SqlVal::F64(1000.0)]
    );
}

#[test]
fn joined_aggregations_group_rows_by_the_joined_table() {
    let built = SelectQuery::<Postgres<Pg16>>::new()
        .from::<Order>()
        .inner_join::<User>(Expr::column::<UserId>().eq(7i64))
        .select((UserName,))
        .group_by(UserName)
        .having(Expr::count_all().gt(1i64))
        .order_by_asc(UserName)
        .build();

    assert_eq!(
        built.sql,
        "SELECT \"name\" FROM \"orders\" INNER JOIN \"users\" ON (\"id\" = $1) \
         GROUP BY \"name\" HAVING (COUNT(*) > $2) ORDER BY \"name\" ASC"
    );
    assert_eq!(built.params, vec![SqlVal::I64(7), SqlVal::I64(1)]);
}

#[test]
fn window_functions_render_over_clause_and_decode_rows() {
    let window = Window::partition_by(OrderUserId).order_by_desc(OrderTotal);

    let built = SelectQuery::<Postgres<Pg16>>::new()
        .from::<Order>()
        .select((
            OrderId,
            row_number().over(window.clone()),
            rank().over(window.clone()),
            dense_rank().over(window.clone()),
            lag(OrderTotal).offset(1).over(Window::order_by_asc(OrderId)),
            lead(OrderTotal).over(Window::order_by_asc(OrderId)),
            Expr::sum::<OrderTotal>().over(Window::partition_by(OrderUserId)),
        ))
        .build();

    assert_eq!(
        built.sql,
        "SELECT \"id\", ROW_NUMBER() OVER (PARTITION BY \"user_id\" ORDER BY \"total\" DESC), \
         RANK() OVER (PARTITION BY \"user_id\" ORDER BY \"total\" DESC), \
         DENSE_RANK() OVER (PARTITION BY \"user_id\" ORDER BY \"total\" DESC), \
         LAG(\"total\", 1) OVER (ORDER BY \"id\" ASC), \
         LEAD(\"total\") OVER (ORDER BY \"id\" ASC), \
         SUM(\"total\") OVER (PARTITION BY \"user_id\") \
         FROM \"orders\""
    );

    let row = <(i64, i64, i64, i64, Option<f64>, Option<f64>, f64)>::from_row(&[
        SqlVal::I64(1),
        SqlVal::I64(1),
        SqlVal::I64(1),
        SqlVal::I64(1),
        SqlVal::Null,
        SqlVal::F64(45.0),
        SqlVal::F64(100.0),
    ])
    .expect("window row decodes");

    assert_eq!(row.0, 1);
    assert_eq!(row.1, 1);
    assert_eq!(row.2, 1);
    assert_eq!(row.3, 1);
    assert_eq!(row.4, None);
    assert_eq!(row.5, Some(45.0));
    assert_eq!(row.6, 100.0);
}

#[test]
fn cte_subquery_composes_with_main_query_and_decodes_rows() {
    let summary_subquery = select_from::<Postgres<Pg16>, Order>()
        .select((OrderUserId, Expr::count_all()))
        .group_by(OrderUserId);

    let summary_cte = summary_subquery.as_cte::<UserSummary>();

    let built = select_from::<Postgres<Pg16>, User>()
        .with(summary_cte)
        .inner_join::<UserSummary>(Expr::column::<UserId>().eq(Expr::column::<SummaryUserId>()))
        .select((UserId, UserName, SummaryOrderCount))
        .where_(Expr::column::<SummaryOrderCount>().gt(5i64))
        .build();

    assert_eq!(
        built.sql,
        "WITH \"user_summary\" AS (SELECT \"user_id\", COUNT(*) FROM \"orders\" GROUP BY \"user_id\") \
         SELECT \"id\", \"name\", \"order_count\" FROM \"users\" \
         INNER JOIN \"user_summary\" ON (\"id\" = \"user_id\") \
         WHERE (\"order_count\" > $1)"
    );
    assert_eq!(built.params, vec![SqlVal::I64(5)]);

    let row = <(i64, String, i64)>::from_row(&[
        SqlVal::I64(1),
        SqlVal::Text("alice".into()),
        SqlVal::I64(10),
    ])
    .expect("cte row decodes");
    assert_eq!(row, (1, "alice".into(), 10));
}

#[test]
fn named_cte_renders_with_clause_and_binds_parameters() {
    let active_orders = select_from::<Postgres<Pg16>, Order>()
        .select((OrderId, OrderUserId))
        .where_(Expr::column::<OrderTotal>().gt(100.0));

    let built = select_from::<Postgres<Pg16>, User>()
        .with(cte_named("active_orders", active_orders))
        .select((UserId, UserName))
        .where_(Expr::column::<UserId>().gt(10i64))
        .build();

    assert_eq!(
        built.sql,
        "WITH \"active_orders\" AS (SELECT \"id\", \"user_id\" FROM \"orders\" WHERE (\"total\" > $1)) \
         SELECT \"id\", \"name\" FROM \"users\" WHERE (\"id\" > $2)"
    );
    assert_eq!(built.params, vec![SqlVal::F64(100.0), SqlVal::I64(10)]);
}

#[test]
fn sql_macro_integrates_into_query_clauses_and_binds_typed_params() {
    let custom_filter = sql!(
        bool,
        "EXTRACT(YEAR FROM created_at) = ? AND status = ?",
        2024i64,
        "active"
    );

    let built = select_from::<Postgres<Pg16>, User>()
        .select((UserId, sql!(<i64> "COUNT(*)")))
        .where_(Expr::column::<UserActive>().eq(true))
        .where_(custom_filter)
        .build();

    assert_eq!(
        built.sql,
        "SELECT \"id\", COUNT(*) FROM \"users\" WHERE ((\"active\" = $1) AND \
         EXTRACT(YEAR FROM created_at) = $2 AND status = $3)"
    );
    assert_eq!(
        built.params,
        vec![
            SqlVal::Bool(true),
            SqlVal::I64(2024),
            SqlVal::Text("active".into()),
        ]
    );

    let row = <(i64, i64)>::from_row(&[SqlVal::I64(1), SqlVal::I64(42)]).expect("decodes");
    assert_eq!(row, (1, 42));
}

#[test]
fn raw_sql_macro_escapes_and_formats_subexpressions() {
    let expr = raw_sql!(bool, "data ?? 'role' AND score >= {}", 90i64);

    let built = select_from::<Postgres<Pg16>, User>()
        .select((UserId,))
        .where_(expr)
        .build();

    assert_eq!(
        built.sql,
        "SELECT \"id\" FROM \"users\" WHERE data ? 'role' AND score >= $1"
    );
    assert_eq!(built.params, vec![SqlVal::I64(90)]);
}

#[test]
fn sql_macro_composes_with_group_by_and_having() {
    let built = select_from::<Postgres<Pg16>, Order>()
        .select((OrderUserId, sql!(<i64> "COUNT(*)")))
        .group_by(OrderUserId)
        .having(sql!(bool, "SUM(total) > ?", 250.0))
        .having(Expr::count_all().lt(100i64))
        .build();

    assert_eq!(
        built.sql,
        "SELECT \"user_id\", COUNT(*) FROM \"orders\" GROUP BY \"user_id\" \
         HAVING (SUM(total) > $1 AND (COUNT(*) < $2))"
    );
    assert_eq!(built.params, vec![SqlVal::F64(250.0), SqlVal::I64(100)]);
}
