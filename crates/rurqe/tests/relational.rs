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
