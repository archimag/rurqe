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
        UserManagerId => "manager_id": i64,
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

table! {
    /// The most recent order per user, produced by a LATERAL subquery.
    RecentOrder => "recent" {
        RecentTotal => "total": f64,
    }
}

table_alias! {
    /// The `manager` alias.
    ManagerAlias => "manager"
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
fn expression_scopes_cover_tables_tuples_and_joined_scopes() {
    fn assert_expr_scope<S: ExprScope>() {}
    fn assert_in_scope<T: Table, S: TableInScope<T, P>, P>() {}
    fn assert_belongs<C: Column + ColumnBelongsTo<T>, T: Table>() {}

    assert_expr_scope::<User>();
    assert_expr_scope::<Order>();
    assert_expr_scope::<(User, Order)>();
    assert_expr_scope::<Joined<User, Order>>();
    assert_expr_scope::<LeftJoined<User, Order>>();
    assert_expr_scope::<Joined<LeftJoined<User, Order>, User>>();

    assert_in_scope::<User, User, _>();
    assert_in_scope::<User, (User, Order), _>();
    assert_in_scope::<Order, (User, Order), _>();
    assert_in_scope::<User, (User, User), At<1>>();
    assert_in_scope::<User, (User, User), At<2>>();

    assert_belongs::<UserId, User>();
    assert_belongs::<OrderId, Order>();
}

#[test]
fn inner_join_spans_multiple_tables_and_decodes_rows() {
    assert_scope::<User>();
    assert_scope::<Joined<User, Order>>();
    assert_scope::<LeftJoined<User, Order>>();

    let selection = cols!(UserId, OrderId, OrderTotal);

    let built = select_from::<Postgres<Pg16>, User>()
        .inner_join::<Order, _, _>(Expr::column::<OrderUserId>().eq(7i64))
        .select(cols!(UserId, OrderId, OrderTotal))
        .where_(Expr::column::<UserActive>().eq(true))
        .build();

    assert_eq!(
        built.sql,
        "SELECT \"users\".\"id\", \"orders\".\"id\", \"orders\".\"total\" FROM \"users\" \
         INNER JOIN \"orders\" ON (\"orders\".\"user_id\" = $1) WHERE (\"users\".\"active\" = $2)"
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
fn cross_join_and_full_outer_join_render_and_lift_nullability() {
    let cross = select_from::<Postgres<Pg16>, User>()
        .cross_join_table(Order)
        .select((UserId, OrderId))
        .build();

    assert_eq!(
        cross.sql,
        "SELECT \"users\".\"id\", \"orders\".\"id\" FROM \"users\" CROSS JOIN \"orders\""
    );

    type FullOuterScope = FullOuterJoined<User, Order>;
    type NullablePair = (Option<i64>, Option<i64>);

    let full_outer: SelectQuery<Postgres<Pg16>, FullOuterScope, NullablePair, HasProjection> =
        select_from::<Postgres<Pg16>, User>()
            .full_outer_join_table(
                Order,
                Expr::column::<UserId>().eq(Expr::column::<OrderUserId>()),
            )
            .select((UserId, OrderId));

    assert_eq!(
        full_outer.build().sql,
        "SELECT \"users\".\"id\", \"orders\".\"id\" FROM \"users\" FULL OUTER JOIN \"orders\" \
         ON (\"users\".\"id\" = \"orders\".\"user_id\")"
    );

    // Bilateral nullability verification: both sides decode as Option<i64>
    let matched = <(Option<i64>, Option<i64>)>::from_row(&[SqlVal::I64(1), SqlVal::I64(10)])
        .expect("matched row decodes");
    assert_eq!(matched, (Some(1), Some(10)));

    let left_only = <(Option<i64>, Option<i64>)>::from_row(&[SqlVal::I64(1), SqlVal::Null])
        .expect("left-only row decodes");
    assert_eq!(left_only, (Some(1), None));

    let right_only = <(Option<i64>, Option<i64>)>::from_row(&[SqlVal::Null, SqlVal::I64(10)])
        .expect("right-only row decodes");
    assert_eq!(right_only, (None, Some(10)));
}

#[test]
fn left_join_lifts_joined_columns_into_options() {
    assert_nullable_row::<(UserId, Nullable<OrderTotal>)>();

    let query = SelectQuery::<Postgres<Pg14>>::new()
        .from::<User>()
        .left_join::<Order, _, _>(Expr::column::<OrderUserId>().eq(3i64));
    let total = query.nullable::<OrderTotal>();
    let built = query.select((UserId, total)).build();

    assert_eq!(
        built.sql,
        "SELECT \"users\".\"id\", \"orders\".\"total\" FROM \"users\" LEFT JOIN \"orders\" \
         ON (\"orders\".\"user_id\" = $1)"
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
fn aggregate_filter_where_and_distinct_aggregates_render_expected_sql() {
    let built = SelectQuery::<Postgres<Pg16>>::new()
        .from::<Order>()
        .select((
            OrderUserId,
            count_distinct(OrderId),
            sum(OrderTotal).filter_where::<Postgres<Pg16>, _>(Expr::column::<OrderTotal>().gt(100.0)),
            avg_distinct(OrderTotal)
                .filter_where::<Postgres<Pg16>, _>(Expr::column::<OrderId>().gt(0i64)),
        ))
        .group_by(OrderUserId)
        .build();

    assert_eq!(
        built.sql,
        "SELECT \"user_id\", COUNT(DISTINCT \"id\"), \
         SUM(\"total\") FILTER (WHERE (\"total\" > $1)), \
         AVG(DISTINCT \"total\") FILTER (WHERE (\"id\" > $2)) \
         FROM \"orders\" GROUP BY \"user_id\""
    );
    assert_eq!(built.params, vec![SqlVal::F64(100.0), SqlVal::I64(0)]);
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
        .inner_join::<User, _, _>(Expr::column::<UserId>().eq(7i64))
        .select((UserName,))
        .group_by(UserName)
        .having(Expr::count_all().gt(1i64))
        .order_by_asc(UserName)
        .build();

    assert_eq!(
        built.sql,
        "SELECT \"users\".\"name\" FROM \"orders\" INNER JOIN \"users\" \
         ON (\"users\".\"id\" = $1) GROUP BY \"users\".\"name\" HAVING (COUNT(*) > $2) \
         ORDER BY \"users\".\"name\" ASC"
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
        .inner_join::<UserSummary, _, _>(
            Expr::column::<UserId>().eq(Expr::column::<SummaryUserId>()),
        )
        .select((UserId, UserName, SummaryOrderCount))
        .where_(Expr::column::<SummaryOrderCount>().gt(5i64))
        .build();

    assert_eq!(
        built.sql,
        "WITH \"user_summary\" AS (SELECT \"user_id\", COUNT(*) FROM \"orders\" GROUP BY \"user_id\") \
         SELECT \"users\".\"id\", \"users\".\"name\", \"user_summary\".\"order_count\" FROM \"users\" \
         INNER JOIN \"user_summary\" ON (\"users\".\"id\" = \"user_summary\".\"user_id\") \
         WHERE (\"user_summary\".\"order_count\" > $1)"
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
fn a_derived_table_replaces_the_from_relation_of_a_query() {
    let counts = select_from::<Postgres<Pg16>, Order>()
        .select((OrderUserId, Expr::count_all()))
        .where_(Expr::column::<OrderTotal>().gt(100.0))
        .group_by(OrderUserId)
        .as_table::<UserSummary>();

    let built = SelectQuery::<Postgres<Pg16>>::new()
        .from_subquery(counts)
        .select((SummaryUserId, SummaryOrderCount))
        .where_(Expr::column::<SummaryUserId>().gt(10i64))
        .order_by_desc(SummaryOrderCount)
        .limit(2)
        .build();

    assert_eq!(
        built.sql,
        "SELECT \"user_id\", \"order_count\" FROM (SELECT \"user_id\", COUNT(*) FROM \"orders\" \
         WHERE (\"total\" > $1) GROUP BY \"user_id\") AS \"user_summary\" \
         WHERE (\"user_id\" > $2) ORDER BY \"order_count\" DESC LIMIT 2"
    );

    // The subquery is rendered before the enclosing filter, so its parameter
    // carries the first placeholder of the statement.
    assert_eq!(built.params, vec![SqlVal::F64(100.0), SqlVal::I64(10)]);

    let record =
        <(i64, i64)>::from_row(&[SqlVal::I64(1), SqlVal::I64(7)]).expect("derived rows decode");
    assert_eq!(record, (1, 7));
}

#[test]
fn a_derived_table_joins_a_stored_relation() {
    let counts = select_from::<Postgres<Pg16>, Order>()
        .select((OrderUserId, Expr::count_all()))
        .group_by(OrderUserId)
        .as_table::<UserSummary>();

    let built = select_from::<Postgres<Pg16>, User>()
        .inner_join_subquery(
            counts,
            Expr::column::<SummaryUserId>().eq(Expr::column::<UserId>()),
        )
        .select((UserId, UserName, SummaryOrderCount))
        .where_(Expr::column::<SummaryOrderCount>().gt(1i64))
        .build();

    assert_eq!(
        built.sql,
        "SELECT \"users\".\"id\", \"users\".\"name\", \"user_summary\".\"order_count\" \
         FROM \"users\" INNER JOIN (SELECT \"user_id\", COUNT(*) FROM \"orders\" \
         GROUP BY \"user_id\") AS \"user_summary\" \
         ON (\"user_summary\".\"user_id\" = \"users\".\"id\") \
         WHERE (\"user_summary\".\"order_count\" > $1)"
    );
    assert_eq!(built.params, vec![SqlVal::I64(1)]);
}

#[test]
fn a_left_joined_derived_table_lifts_its_columns_into_options() {
    let counts = select_from::<Postgres<Pg16>, Order>()
        .select((OrderUserId, Expr::count_all()))
        .group_by(OrderUserId)
        .as_table::<UserSummary>();

    // The projected row type states that the derived columns reach the row as
    // `Option<i64>`, exactly like the columns of a left-joined stored table. Only
    // the row type is pinned: the scope follows from the join and the projection
    // state from the selection, so spelling them out would state the same twice.
    let query: SelectQuery<_, _, (i64, Option<i64>), _> = select_from::<Postgres<Pg16>, User>()
        .left_join_subquery(
            counts,
            Expr::column::<SummaryUserId>().eq(Expr::column::<UserId>()),
        )
        .select((UserId, SummaryOrderCount));

    let built = query.build();

    assert_eq!(
        built.sql,
        "SELECT \"users\".\"id\", \"user_summary\".\"order_count\" FROM \"users\" \
         LEFT JOIN (SELECT \"user_id\", COUNT(*) FROM \"orders\" GROUP BY \"user_id\") \
         AS \"user_summary\" ON (\"user_summary\".\"user_id\" = \"users\".\"id\")"
    );
    assert!(built.params.is_empty());

    let missing = <(i64, Option<i64>)>::from_row(&[SqlVal::I64(1), SqlVal::Null])
        .expect("unmatched rows decode");
    assert_eq!(missing, (1, None));
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

#[test]
fn null_predicates_and_case_expressions_render_typed_sql() {
    let tier = Expr::<String>::case()
        .when(Expr::column::<OrderTotal>().gt(100.0), "large")
        .when(Expr::column::<OrderTotal>().gt(10.0), "medium")
        .else_("small")
        .end();

    let built = SelectQuery::<Postgres<Pg16>>::new()
        .from::<Order>()
        .select((OrderId, tier))
        .where_(Expr::column::<OrderTotal>().is_not_null())
        .build();

    assert_eq!(
        built.sql,
        "SELECT \"id\", CASE WHEN (\"total\" > $1) THEN $2 WHEN (\"total\" > $3) THEN $4 \
         ELSE $5 END FROM \"orders\" WHERE (\"total\" IS NOT NULL)"
    );
    assert_eq!(
        built.params,
        vec![
            SqlVal::F64(100.0),
            SqlVal::Text("large".into()),
            SqlVal::F64(10.0),
            SqlVal::Text("medium".into()),
            SqlVal::Text("small".into()),
        ]
    );

    let nulls = SelectQuery::<Postgres<Pg16>>::new()
        .from::<Order>()
        .select((OrderId,))
        .where_(Expr::column::<OrderTotal>().is_null())
        .build();

    assert_eq!(
        nulls.sql,
        "SELECT \"id\" FROM \"orders\" WHERE (\"total\" IS NULL)"
    );
    assert!(nulls.params.is_empty());

    let row = <(i64, String)>::from_row(&[SqlVal::I64(7), SqlVal::Text("medium".into())])
        .expect("case rows decode");
    assert_eq!(row, (7, "medium".to_owned()));
}

#[test]
fn subquery_predicates_filter_rows_and_bind_parameters_in_order() {
    let big_spenders = select_from::<Postgres<Pg16>, Order>()
        .select((OrderUserId,))
        .where_(Expr::column::<OrderTotal>().gt(100.0));

    let built = select_from::<Postgres<Pg16>, User>()
        .select((UserId, UserName))
        .where_(Expr::column::<UserId>().in_subquery(big_spenders))
        .where_(Expr::not_exists(
            select_from::<Postgres<Pg16>, Order>()
                .select((OrderId,))
                .where_(Expr::column::<OrderTotal>().lt(5.0)),
        ))
        .build();

    assert_eq!(
        built.sql,
        "SELECT \"id\", \"name\" FROM \"users\" WHERE ((\"id\" IN (SELECT \"user_id\" FROM \
         \"orders\" WHERE (\"total\" > $1))) AND (NOT EXISTS (SELECT \"id\" FROM \"orders\" \
         WHERE (\"total\" < $2))))"
    );
    assert_eq!(built.params, vec![SqlVal::F64(100.0), SqlVal::F64(5.0)]);
}

/// Builds a `CASE`-labelled customer query guarded by a correlated `EXISTS` predicate.
fn active_customers() -> SelectQuery<Postgres<Pg16>, User, (i64, String), HasProjection> {
    select_from::<Postgres<Pg16>, User>()
        .select((
            UserId,
            Expr::<String>::case()
                .when(Expr::column::<UserActive>().eq(true), "active")
                .else_("inactive")
                .end(),
        ))
        .where_(Expr::exists(
            select_from::<Postgres<Pg16>, Order>()
                .select((OrderId,))
                .where_(Expr::column::<OrderUserId>().eq(Expr::column::<UserId>().without_scope())),
        ))
}

#[test]
fn set_operations_combine_case_and_exists_queries() {
    let built = active_customers().union_all(active_customers()).build();

    assert_eq!(
        built.sql,
        "SELECT \"id\", CASE WHEN (\"active\" = $1) THEN $2 ELSE $3 END FROM \"users\" \
         WHERE (EXISTS (SELECT \"id\" FROM \"orders\" WHERE (\"user_id\" = \"users\".\"id\"))) \
         UNION ALL SELECT \"id\", CASE WHEN (\"active\" = $4) THEN $5 ELSE $6 END FROM \"users\" \
         WHERE (EXISTS (SELECT \"id\" FROM \"orders\" WHERE (\"user_id\" = \"users\".\"id\")))"
    );
    assert_eq!(
        built.params,
        vec![
            SqlVal::Bool(true),
            SqlVal::Text("active".into()),
            SqlVal::Text("inactive".into()),
            SqlVal::Bool(true),
            SqlVal::Text("active".into()),
            SqlVal::Text("inactive".into()),
        ]
    );

    fn base() -> SelectQuery<Postgres<Pg16>, Order, (i64,), HasProjection> {
        select_from::<Postgres<Pg16>, Order>().select((OrderUserId,))
    }

    assert_eq!(
        base().intersect(base()).build().sql,
        "SELECT \"user_id\" FROM \"orders\" INTERSECT SELECT \"user_id\" FROM \"orders\""
    );
    assert_eq!(
        base().except(base()).build().sql,
        "SELECT \"user_id\" FROM \"orders\" EXCEPT SELECT \"user_id\" FROM \"orders\""
    );
}

/// Builds the most recent order of the outer `users` row.
fn latest_order_per_user() -> SelectQuery<Postgres<Pg16>, Order, (f64,), HasProjection> {
    select_from::<Postgres<Pg16>, Order>()
        .select((OrderTotal,))
        .where_(Expr::column::<OrderUserId>().eq(Expr::column::<UserId>().without_scope()))
        .order_by_desc(OrderId)
        .limit(1)
}

#[test]
fn self_join_with_aliases_qualifies_repeated_table_references() {
    let built = select_from::<Postgres<Pg16>, User>()
        .inner_join::<Aliased<User, ManagerAlias>, _, _>(
            Expr::column::<UserManagerId>().eq(Expr::column::<AliasedCol<ManagerAlias, UserId>>()),
        )
        .select((
            UserId,
            UserName,
            AliasedCol::<ManagerAlias, UserId>::new(),
            AliasedCol::<ManagerAlias, UserName>::new(),
        ))
        .build();

    assert_eq!(
        built.sql,
        "SELECT \"users\".\"id\", \"users\".\"name\", \"manager\".\"id\", \"manager\".\"name\" \
         FROM \"users\" INNER JOIN \"users\" AS \"manager\" \
         ON (\"users\".\"manager_id\" = \"manager\".\"id\")"
    );
    assert!(built.params.is_empty());
}

#[test]
fn cross_join_lateral_renders_correlated_subquery() {
    let built = select_from::<Postgres<Pg16>, User>()
        .cross_join_lateral(latest_order_per_user().as_lateral::<RecentOrder>())
        .select((UserId, RecentTotal))
        .build();

    assert_eq!(
        built.sql,
        "SELECT \"users\".\"id\", \"recent\".\"total\" FROM \"users\" CROSS JOIN LATERAL \
         (SELECT \"total\" FROM \"orders\" WHERE (\"user_id\" = \"users\".\"id\") \
         ORDER BY \"id\" DESC LIMIT 1) AS \"recent\""
    );
    assert!(built.params.is_empty());
}

#[test]
fn left_join_lateral_lifts_columns_into_options() {
    let query = select_from::<Postgres<Pg16>, User>()
        .left_join_lateral(latest_order_per_user().as_lateral::<RecentOrder>());
    let total = query.nullable::<RecentTotal>();

    let built = query.select((UserId, total)).build();

    assert_eq!(
        built.sql,
        "SELECT \"users\".\"id\", \"recent\".\"total\" FROM \"users\" LEFT JOIN LATERAL \
         (SELECT \"total\" FROM \"orders\" WHERE (\"user_id\" = \"users\".\"id\") \
         ORDER BY \"id\" DESC LIMIT 1) AS \"recent\" ON true"
    );
    assert!(built.params.is_empty());
}

#[test]
fn scope_invariants_accept_only_tables_in_scope() {
    fn assert_in_scope<T: Table, S: TableInScope<T, P>, P>() {}
    fn assert_selection_in_scope<S, T, P>()
    where
        S: SelectionInScope<T, P>,
    {
    }
    fn assert_group_by_in_scope<S, T, P>()
    where
        S: GroupByInScope<T, P>,
    {
    }

    // Joined scopes keep every participating table reachable at a distinct position.
    assert_in_scope::<User, Joined<User, Order>, Next<At<0>>>();
    assert_in_scope::<Order, Joined<User, Order>, At<0>>();
    assert_in_scope::<User, LeftJoined<User, Order>, Next<At<0>>>();

    // Projections and grouping of in-scope tables are accepted without turbofish noise.
    assert_selection_in_scope::<(UserId, OrderTotal), Joined<User, Order>, _>();
    assert_group_by_in_scope::<(UserId, OrderUserId), Joined<User, Order>, _>();
}

/// End-to-end verification of the turbofish-free, value-based SQL DSL.
///
/// The module imports the PostgreSQL dialect prelude only: the dialect is fixed
/// by the prelude, tables are passed as zero-sized values, and neither the
/// queries nor their DML counterparts mention `Postgres` or a turbofish.
#[test]
fn a_scalar_subquery_compares_without_an_explicit_conversion() {
    // A single-column query projects exactly one value, so a comparison operator
    // accepts it directly and no conversion method is spelled out.
    let average = select_from::<Postgres<Pg16>, Order>()
        .select((Expr::avg::<OrderTotal>(),))
        .where_(Expr::column::<OrderId>().gt(0i64));

    let built = select_from::<Postgres<Pg16>, Order>()
        .select((OrderId,))
        .where_(Expr::column::<OrderTotal>().gt(average))
        .build();

    assert_eq!(
        built.sql,
        "SELECT \"id\" FROM \"orders\" WHERE (\"total\" > \
         (SELECT AVG(\"total\") FROM \"orders\" WHERE (\"id\" > $1)))"
    );
    assert_eq!(built.params, vec![SqlVal::I64(0)]);
}

#[test]
fn a_row_value_expression_compares_a_tuple_as_one_value() {
    let built = select_from::<Postgres<Pg16>, Order>()
        .select((OrderId,))
        .where_((OrderUserId, OrderId).gte((2i64, 10i64)))
        .build();

    assert_eq!(
        built.sql,
        "SELECT \"id\" FROM \"orders\" WHERE ((\"user_id\", \"id\") >= ($1, $2))"
    );
    assert_eq!(built.params, vec![SqlVal::I64(2), SqlVal::I64(10)]);
}

#[test]
fn a_row_value_expression_matches_the_rows_of_a_subquery() {
    let pairs = select_from::<Postgres<Pg16>, Order>()
        .select((OrderUserId, OrderId))
        .where_(Expr::column::<OrderTotal>().gt(100.0));

    let built = select_from::<Postgres<Pg16>, Order>()
        .select((OrderId,))
        .where_((OrderUserId, OrderId).in_subquery(pairs))
        .build();

    assert_eq!(
        built.sql,
        "SELECT \"id\" FROM \"orders\" WHERE ((\"user_id\", \"id\") IN \
         (SELECT \"user_id\", \"id\" FROM \"orders\" WHERE (\"total\" > $1)))"
    );
    assert_eq!(built.params, vec![SqlVal::F64(100.0)]);
}

#[test]
fn a_keyset_cursor_walks_a_row_value_ordering() {
    // The cursor is a row value, so the page after it is one tuple comparison
    // over the ordered pair instead of a growing chain of `OR` predicates.
    let built = select_from::<Postgres<Pg16>, Order>()
        .select((OrderId, OrderUserId))
        .where_((OrderUserId, OrderId).lt((2i64, 40i64)))
        .order_by_desc(OrderUserId)
        .order_by_desc(OrderId)
        .limit(10)
        .build();

    assert_eq!(
        built.sql,
        "SELECT \"id\", \"user_id\" FROM \"orders\" \
         WHERE ((\"user_id\", \"id\") < ($1, $2)) \
         ORDER BY \"user_id\" DESC, \"id\" DESC LIMIT 10"
    );
    assert_eq!(built.params, vec![SqlVal::I64(2), SqlVal::I64(40)]);
}

#[test]
fn a_query_locks_the_rows_it_claims_until_the_transaction_ends() {
    let built = select_from::<Postgres<Pg16>, Order>()
        .select((OrderId,))
        .where_(Expr::column::<OrderTotal>().gt(10.0))
        .order_by_asc(OrderId)
        .limit(1)
        .for_update()
        .skip_locked()
        .build();

    assert_eq!(
        built.sql,
        "SELECT \"id\" FROM \"orders\" WHERE (\"total\" > $1) \
         ORDER BY \"id\" ASC LIMIT 1 FOR UPDATE SKIP LOCKED"
    );
    assert_eq!(built.params, vec![SqlVal::F64(10.0)]);

    let shared = SelectQuery::<Postgres<Pg14>>::new()
        .from::<Order>()
        .select((OrderId,))
        .for_share()
        .nowait()
        .build();

    assert_eq!(shared.sql, "SELECT \"id\" FROM \"orders\" FOR SHARE NOWAIT");
    assert!(shared.params.is_empty());
}

#[test]
fn window_frames_restrict_the_rows_a_window_reads() {
    let built = SelectQuery::<Postgres<Pg16>>::new()
        .from::<Order>()
        .select((
            OrderId,
            Expr::sum::<OrderTotal>().over(
                Window::partition_by(OrderUserId)
                    .order_by_asc(OrderId)
                    .rows_between(
                        WindowFrameBound::UnboundedPreceding,
                        WindowFrameBound::CurrentRow,
                    ),
            ),
            dense_rank().over(
                Window::order_by_desc(OrderTotal).range_between(
                    WindowFrameBound::preceding(1),
                    WindowFrameBound::following(1),
                ),
            ),
        ))
        .build();

    assert_eq!(
        built.sql,
        "SELECT \"id\", SUM(\"total\") OVER (PARTITION BY \"user_id\" ORDER BY \"id\" ASC \
         ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW), \
         DENSE_RANK() OVER (ORDER BY \"total\" DESC \
         RANGE BETWEEN 1 PRECEDING AND 1 FOLLOWING) FROM \"orders\""
    );
    assert!(built.params.is_empty());
}

#[test]
fn a_filtered_aggregate_restricts_the_rows_it_reads() {
    let spent =
        sum(OrderTotal).filter_where::<Postgres<Pg16>, _>(Expr::column::<OrderId>().gt(0i64));

    let built = SelectQuery::<Postgres<Pg16>>::new()
        .from::<Order>()
        .select((OrderUserId, spent.clone()))
        .group_by(OrderUserId)
        .having(spent.gt(500.0))
        .build();

    assert_eq!(
        built.sql,
        "SELECT \"user_id\", SUM(\"total\") FILTER (WHERE (\"id\" > $1)) FROM \"orders\" \
         GROUP BY \"user_id\" HAVING (SUM(\"total\") FILTER (WHERE (\"id\" > $2)) > $3)"
    );
    assert_eq!(
        built.params,
        vec![SqlVal::I64(0), SqlVal::I64(0), SqlVal::F64(500.0)]
    );
}

mod value_based_dsl {
    use rurqe::postgres::prelude::*;

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

    #[test]
    fn joins_render_without_table_or_dialect_turbofish() {
        let built = select_from(User)
            .inner_join_table(Order, OrderUserId.eq(Expr::column::<UserId>()))
            .select(cols!(UserId, UserName, OrderTotal))
            .where_(UserId.gt(10i64))
            .order_by_asc(UserId)
            .limit(20)
            .build();

        assert_eq!(
            built.sql,
            "SELECT \"users\".\"id\", \"users\".\"name\", \"orders\".\"total\" FROM \"users\" \
             INNER JOIN \"orders\" ON (\"orders\".\"user_id\" = \"users\".\"id\") \
             WHERE (\"users\".\"id\" > $1) ORDER BY \"users\".\"id\" ASC LIMIT 20"
        );
        assert_eq!(built.params, vec![SqlVal::I64(10)]);
    }

    #[test]
    fn left_joined_columns_lift_into_options_without_explicit_markers() {
        let built = select_from(User)
            .left_join_table(Order, OrderUserId.eq(Expr::column::<UserId>()))
            .select(cols!(UserId, OrderTotal))
            .build();

        assert_eq!(
            built.sql,
            "SELECT \"users\".\"id\", \"orders\".\"total\" FROM \"users\" \
             LEFT JOIN \"orders\" ON (\"orders\".\"user_id\" = \"users\".\"id\")"
        );
        assert!(built.params.is_empty());

        let missing = <(i64, Option<f64>)>::from_row(&[SqlVal::I64(1), SqlVal::Null])
            .expect("unmatched rows decode");
        assert_eq!(missing, (1, None));
    }

    #[test]
    fn grouped_windows_and_set_operations_stay_dialect_free() {
        let grouped = select_from(Order)
            .group_by(OrderUserId)
            .select((OrderUserId, Expr::count_all()))
            .having(Expr::count_all().gt(1i64))
            .order_by_desc(OrderUserId)
            .build();

        assert_eq!(
            grouped.sql,
            "SELECT \"user_id\", COUNT(*) FROM \"orders\" GROUP BY \"user_id\" \
             HAVING (COUNT(*) > $1) ORDER BY \"user_id\" DESC"
        );
        assert_eq!(grouped.params, vec![SqlVal::I64(1)]);

        let ranked = select_from(Order)
            .select((
                OrderId,
                row_number().over(Window::partition_by(OrderUserId).order_by_desc(OrderTotal)),
            ))
            .build();

        assert_eq!(
            ranked.sql,
            "SELECT \"id\", ROW_NUMBER() OVER (PARTITION BY \"user_id\" ORDER BY \"total\" DESC) \
             FROM \"orders\""
        );

        fn active() -> SelectQuery<Postgres, User, (i64,), HasProjection> {
            select_from(User).select((UserId,)).where_(UserActive.eq(true))
        }

        assert_eq!(
            active().union_all(active()).build().sql,
            "SELECT \"id\" FROM \"users\" WHERE (\"active\" = $1) \
             UNION ALL SELECT \"id\" FROM \"users\" WHERE (\"active\" = $2)"
        );
    }

    #[test]
    fn dml_entrypoints_take_table_values() {
        let inserted = insert_into(User)
            .values((UserId::set(1i64), UserName::set("root")))
            .build();

        assert_eq!(
            inserted.sql,
            "INSERT INTO \"users\" (\"id\", \"name\") VALUES ($1, $2)"
        );
        assert_eq!(
            inserted.params,
            vec![SqlVal::I64(1), SqlVal::Text("root".to_owned())]
        );

        let updated = update(User)
            .set(UserActive::set(false))
            .where_(UserId.eq(1i64))
            .build();

        assert_eq!(
            updated.sql,
            "UPDATE \"users\" SET \"active\" = $1 WHERE (\"id\" = $2)"
        );
        assert_eq!(updated.params, vec![SqlVal::Bool(false), SqlVal::I64(1)]);

        let deleted = delete_from(User)
            .where_(UserId.eq(1i64))
            .returning((UserName,))
            .build();

        assert_eq!(
            deleted.sql,
            "DELETE FROM \"users\" WHERE (\"id\" = $1) RETURNING \"name\""
        );
        assert_eq!(deleted.params, vec![SqlVal::I64(1)]);
    }
}

#[test]
fn an_optional_select_predicate_keeps_the_query_renderable() {
    fn build(name: Option<&str>, only_active: bool) -> BuiltQuery {
        select_from::<Postgres<Pg16>, User>()
            .select((UserId,))
            .when(only_active, |query| {
                query.where_(Expr::column::<UserActive>().eq(true))
            })
            .if_some(name, |query, name| {
                query.where_(Expr::column::<UserName>().eq(name))
            })
            .build()
    }

    let filtered = build(Some("alice"), true);

    assert_eq!(
        filtered.sql,
        "SELECT \"id\" FROM \"users\" WHERE ((\"active\" = $1) AND (\"name\" = $2))"
    );
    assert_eq!(
        filtered.params,
        vec![SqlVal::Bool(true), SqlVal::Text("alice".to_owned())]
    );

    let unfiltered = build(None, false);

    assert_eq!(unfiltered.sql, "SELECT \"id\" FROM \"users\"");
    assert!(unfiltered.params.is_empty());
}

#[test]
fn an_absent_optional_select_predicate_leaves_the_query_unfiltered() {
    let built = select_from::<Postgres<Pg16>, User>()
        .select((UserId,))
        .where_opt(None::<Expr<bool, User>>)
        .build();

    assert_eq!(built.sql, "SELECT \"id\" FROM \"users\"");
    assert!(built.params.is_empty());
}

#[test]
fn an_optional_mutation_predicate_keeps_the_statement_renderable() {
    let updated = update::<Postgres<Pg16>, User>()
        .set(UserName::set("root"))
        .where_opt(None::<Expr<bool, User>>)
        .build();

    assert_eq!(updated.sql, "UPDATE \"users\" SET \"name\" = $1");
    assert_eq!(updated.params, vec![SqlVal::Text("root".to_owned())]);

    let filtered = update::<Postgres<Pg16>, User>()
        .set(UserName::set("root"))
        .where_opt(Some(Expr::column::<UserId>().eq(1i64)))
        .build();

    assert_eq!(
        filtered.sql,
        "UPDATE \"users\" SET \"name\" = $1 WHERE (\"id\" = $2)"
    );
    assert_eq!(
        filtered.params,
        vec![SqlVal::Text("root".to_owned()), SqlVal::I64(1)]
    );

    let deleted = delete_from::<Postgres<Pg16>, User>()
        .where_opt(None::<Expr<bool, User>>)
        .build();

    assert_eq!(deleted.sql, "DELETE FROM \"users\"");
    assert!(deleted.params.is_empty());
}

/// End-to-end verification of the relational surface delivered as one group in
/// Phase 18.
///
/// The module drives the features that were added together: dynamic query
/// composition, a derived table, a database-first enum, the native array
/// operators of PostgreSQL, and a nested collection read through a JSON
/// aggregate.
#[cfg(feature = "serde_json")]
mod phase18_relational {
    use rurqe::prelude::*;

    /// The lifecycle state of a post, stored as the label of a database enum.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, SqlType)]
    pub enum Mood {
        Happy,
        #[sql(rename = "very_sad")]
        Sad,
    }

    table! {
        /// The `posts` table carrying an enum column and two array columns.
        Post => "posts" {
            PostId => "id": i64,
            PostMood => "mood": Mood,
            PostTags => "tags": Vec<String>,
            PostScores => "scores": Option<Vec<i64>>,
        }
    }

    table! {
        /// The `comments` table read as the nested collection of its post.
        Comment => "comments" {
            CommentPostId => "post_id": i64,
            CommentBody => "body": String,
        }
    }

    table! {
        /// The number of comments of one post, produced by a derived table.
        CommentCount => "comment_count" {
            CountPostId => "post_id": i64,
            CountTotal => "total": i64,
        }
    }

    dto! {
        /// One nested comment row decoded from the JSON aggregate of its post.
        ///
        /// The declaration carries the nested-row contracts, so the row is read
        /// back from the object the aggregate produces without a hand-written
        /// conversion, and the key it reads is the field the nested statement
        /// projects under.
        struct CommentRow {
            CommentBody as body,
        }
    }

    #[test]
    fn a_nested_collection_composes_with_enum_and_array_predicates() {
        let built = select_from::<Postgres<Pg16>, Post>()
            .select((
                PostId,
                nest_many(
                    SelectQuery::<Postgres<Pg16>>::new()
                        .from::<Comment>()
                        .select(select!(CommentBody as body)),
                ),
            ))
            .where_(PostMood.eq(Mood::Happy))
            .where_(PostTags.any("rust".to_owned()))
            .order_by_asc(PostId)
            .limit(5)
            .build();

        assert_eq!(
            built.sql,
            "SELECT \"id\", COALESCE((SELECT jsonb_agg(to_jsonb(\"__rurqe_sub\")) FROM \
             (SELECT \"body\" AS \"body\" FROM \"comments\") AS \"__rurqe_sub\"), '[]'::jsonb) \
             FROM \"posts\" WHERE ((\"mood\" = $1) AND ($2 = ANY(\"tags\"))) \
             ORDER BY \"id\" ASC LIMIT 5"
        );
        assert_eq!(
            built.params,
            vec![
                SqlVal::Text("Happy".to_owned()),
                SqlVal::Text("rust".to_owned()),
            ]
        );

        let rows = Vec::<CommentRow>::from_sql_value(&SqlVal::Text(
            "[{\"body\":\"first\"},{\"body\":\"second\"}]".to_owned(),
        ))
        .expect("the nested collection decodes");

        assert_eq!(
            rows,
            vec![
                CommentRow {
                    body: "first".to_owned(),
                },
                CommentRow {
                    body: "second".to_owned(),
                },
            ]
        );
    }

    #[test]
    fn a_single_nested_row_stops_at_its_first_row() {
        let built = select_from::<Postgres<Pg16>, Post>()
            .select((
                PostId,
                nest_one(
                    SelectQuery::<Postgres<Pg16>>::new()
                        .from::<Comment>()
                        .select(select!(CommentBody as body)),
                ),
            ))
            .build();

        assert_eq!(
            built.sql,
            "SELECT \"id\", (SELECT to_jsonb(\"__rurqe_sub\") FROM (SELECT \"body\" AS \"body\" \
             FROM \"comments\" LIMIT 1) AS \"__rurqe_sub\") FROM \"posts\""
        );
        assert!(built.params.is_empty());

        assert_eq!(
            Option::<CommentRow>::from_sql_value(&SqlVal::Text("{\"body\":\"only\"}".to_owned())),
            Ok(Some(CommentRow {
                body: "only".to_owned(),
            }))
        );
        assert_eq!(Option::<CommentRow>::from_sql_value(&SqlVal::Null), Ok(None));
    }

    #[test]
    fn a_dynamic_query_joins_a_derived_table_and_filters_on_arrays() {
        let counts = select_from::<Postgres<Pg16>, Comment>()
            .select((CommentPostId, Expr::count_all()))
            .group_by(CommentPostId)
            .as_table::<CommentCount>();

        // An absent predicate must leave the statement untouched. The predicate
        // reads a column of one table of the joined scope, so its scope names
        // that table instead of the join that introduced it.
        let absent: Option<Expr<bool, Post>> = None;

        let built = select_from::<Postgres<Pg16>, Post>()
            .inner_join_subquery(
                counts,
                Expr::column::<CountPostId>().eq(Expr::column::<PostId>()),
            )
            .select((PostId, CountTotal))
            .when(true, |query| query.where_(PostMood.eq(Mood::Happy)))
            .if_some(Some("rust"), |query, tag| {
                query.where_(PostTags.any(tag.to_owned()))
            })
            .where_opt(absent)
            .order_by_desc(CountTotal)
            .build();

        assert_eq!(
            built.sql,
            "SELECT \"posts\".\"id\", \"comment_count\".\"total\" FROM \"posts\" \
             INNER JOIN (SELECT \"post_id\", COUNT(*) FROM \"comments\" GROUP BY \"post_id\") \
             AS \"comment_count\" ON (\"comment_count\".\"post_id\" = \"posts\".\"id\") \
             WHERE ((\"posts\".\"mood\" = $1) AND ($2 = ANY(\"posts\".\"tags\"))) \
             ORDER BY \"comment_count\".\"total\" DESC"
        );
        assert_eq!(
            built.params,
            vec![
                SqlVal::Text("Happy".to_owned()),
                SqlVal::Text("rust".to_owned()),
            ]
        );
    }

    #[test]
    fn native_array_operators_and_enum_labels_render_on_postgres() {
        let built = select_from::<Postgres<Pg16>, Post>()
            .select((PostId, PostTags, PostScores))
            .where_(PostTags.array_contains(vec!["rust".to_owned()]))
            .where_(PostScores.array_overlaps(vec![1i64, 2i64]))
            .where_(PostMood.eq("very_sad"))
            .build();

        assert_eq!(
            built.sql,
            "SELECT \"id\", \"tags\", \"scores\" FROM \"posts\" WHERE \
             (((\"tags\" @> $1) AND (\"scores\" && $2)) AND (\"mood\" = $3))"
        );
        assert_eq!(
            built.params,
            vec![
                SqlVal::Array(vec![SqlVal::Text("rust".to_owned())]),
                SqlVal::Array(vec![SqlVal::I64(1), SqlVal::I64(2)]),
                SqlVal::Text("very_sad".to_owned()),
            ]
        );

        let quantifier = select_from::<Postgres<Pg16>, Post>()
            .select((PostId,))
            .where_(PostScores.all(0i64))
            .build();

        assert_eq!(
            quantifier.sql,
            "SELECT \"id\" FROM \"posts\" WHERE ($1 = ALL(\"scores\"))"
        );
        assert_eq!(quantifier.params, vec![SqlVal::I64(0)]);

        let row = <(i64, Vec<String>, Option<Vec<i64>>, Mood)>::from_row(&[
            SqlVal::I64(7),
            SqlVal::Array(vec![SqlVal::Text("rust".to_owned())]),
            SqlVal::Null,
            SqlVal::Text("very_sad".to_owned()),
        ])
        .expect("the row decodes");

        assert_eq!(row.0, 7);
        assert_eq!(row.1, vec!["rust".to_owned()]);
        assert_eq!(row.2, None);
        assert_eq!(row.3, Mood::Sad);
    }
}
