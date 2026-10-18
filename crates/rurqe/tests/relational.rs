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

    let selection = select!(UserId, OrderId, OrderTotal);

    let built = SelectQuery::<Postgres<Pg16>>::new()
        .from::<User>()
        .inner_join::<Order>(Expr::column::<OrderUserId>().eq(7i64))
        .select(select!(UserId, OrderId, OrderTotal))
        .where_(Expr::column::<UserActive>().eq(true))
        .build();

    assert_eq!(
        built.sql,
        "SELECT \"id\", \"id\", \"total\" FROM \"users\" INNER JOIN \"orders\" \
         ON (\"user_id\" = $1) WHERE (\"active\" = $2)"
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
