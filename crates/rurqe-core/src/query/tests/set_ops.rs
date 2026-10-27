//! Set operations and the ordering of the combined rows.

use super::{
    compound_user_query, MockBackend, MockDialect, Order, OrderAlias, OrderId, OrderTotal,
    OrderUserId, User, UserId, UserName,
};
use crate::expr::{Expr, OrderCriterion, SqlVal};
use crate::query::*;
use crate::schema::AliasedCol;

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
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "SELECT \"id\", \"name\" FROM \"users\" WHERE (\"id\" < $1) \
         UNION SELECT \"id\", \"name\" FROM \"users\" WHERE (\"id\" > $2)"
    );
    assert_eq!(built.params, vec![SqlVal::I64(5), SqlVal::I64(10)]);
}

#[test]
fn set_operation_keywords_render_expected_sql() {
    fn base() -> SelectQuery<MockDialect, User, (i64,), HasProjection> {
        SelectQuery::<MockDialect>::new()
            .from::<User>()
            .select((UserId,))
    }

    assert_eq!(
        base().union(base()).build().expect("valid query").sql,
        "SELECT \"id\" FROM \"users\" UNION SELECT \"id\" FROM \"users\""
    );
    assert_eq!(
        base().union_all(base()).build().expect("valid query").sql,
        "SELECT \"id\" FROM \"users\" UNION ALL SELECT \"id\" FROM \"users\""
    );
    assert_eq!(
        base().intersect(base()).build().expect("valid query").sql,
        "SELECT \"id\" FROM \"users\" INTERSECT SELECT \"id\" FROM \"users\""
    );
    assert_eq!(
        base().except(base()).build().expect("valid query").sql,
        "SELECT \"id\" FROM \"users\" EXCEPT SELECT \"id\" FROM \"users\""
    );
}

#[test]
fn chained_set_operations_keep_trailing_order_by_and_limit_on_the_union() {
    fn base() -> SelectQuery<MockDialect, User, (i64,), HasProjection> {
        SelectQuery::<MockDialect>::new()
            .from::<User>()
            .select((UserId,))
    }

    let built = base()
        .union(base())
        .union_all(base())
        .order_by_output_asc(UserId)
        .limit(3)
        .build()
        .expect("valid query");

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
    let query_built = query.build().expect("valid query");

    let mut backend = MockBackend {
        rows: vec![vec![SqlVal::I64(1)]],
        ..MockBackend::default()
    };

    let rows = backend.fetch_all(&query).expect("rows decode");
    assert_eq!(rows, vec![(1i64,)]);
    assert_eq!(backend.executed, vec![query_built]);
}

#[test]
fn a_compound_ordering_reads_the_output_columns_of_the_projection() {
    // The ordering of a set operation sorts the combined rows, so the columns it
    // names are the output columns of the projections. A reference qualified with
    // the table of the left query would name a relation the combined statement no
    // longer declares, which PostgreSQL rejects with a `missing FROM-clause entry`
    // error.
    let joined = SelectQuery::<MockDialect>::new()
        .from::<User>()
        .inner_join(Order, Expr::column::<UserId>().eq(Expr::column::<OrderUserId>()))
        .select((UserId, OrderTotal));

    let plain = SelectQuery::<MockDialect>::new()
        .from::<Order>()
        .select((OrderId, OrderTotal));

    let built = joined
        .union(plain)
        .order_by_output_asc(UserId)
        .limit(3)
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "SELECT \"users\".\"id\", \"orders\".\"total\" FROM \"users\" INNER JOIN \"orders\" \
         ON (\"users\".\"id\" = \"orders\".\"user_id\") UNION SELECT \"id\", \"total\" FROM \
         \"orders\" ORDER BY \"id\" ASC LIMIT 3"
    );
    assert!(built.params.is_empty());
}

#[test]
fn a_compound_ordering_renders_a_position_and_an_output_column() {
    let built = compound_user_query()
        .union(compound_user_query())
        .order_by_index(2, OrderDirection::Desc)
        .order_by_output_asc(UserName)
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "SELECT \"id\", \"name\" FROM \"users\" UNION SELECT \"id\", \"name\" FROM \"users\" \
         ORDER BY 2 DESC, \"name\" ASC"
    );
    assert!(built.params.is_empty());
}

#[test]
fn a_compound_ordering_position_outside_the_projection_is_rejected() {
    assert_eq!(
        compound_user_query()
            .union(compound_user_query())
            .order_by_index_asc(3)
            .build(),
        Err(BuildError::CompoundOrderIndex {
            index: 3,
            columns: 2,
        })
    );
    assert_eq!(
        compound_user_query()
            .union(compound_user_query())
            .order_by_index_asc(0)
            .build(),
        Err(BuildError::CompoundOrderIndex {
            index: 0,
            columns: 2,
        })
    );
}

#[test]
fn a_compound_ordering_by_an_aggregate_expression_is_rejected() {
    // The compound ordering builders take a column marker or a position, so an
    // aggregate reaches the clause through the internal ordering recorder. The
    // engine rejects `ORDER BY COUNT(*)` after `UNION` as an invalid compound
    // ordering, so the clause is rejected while building as well.
    let mut query = compound_user_query().union(compound_user_query());
    query.push_ordering(OrderCriterion::new(
        Expr::<i64>::count_all().into_kind(),
        OrderDirection::Asc,
    ));

    assert_eq!(query.build(), Err(BuildError::CompoundOrderExpression));
}

#[test]
fn a_compound_ordering_of_an_unprojected_output_column_is_rejected() {
    // The ordering reads the output column names of the projections, so a column
    // whose name the projection list does not produce cannot be ordered by, even
    // when it is a column of a table the left query selects from.
    assert_eq!(
        compound_user_query()
            .union(compound_user_query())
            .order_by_output_asc(OrderTotal)
            .build(),
        Err(BuildError::CompoundOrderColumn { column: "total" })
    );
}

#[test]
fn a_compound_ordering_accepts_an_aliased_output_column() {
    let left = SelectQuery::<MockDialect>::new()
        .from::<crate::schema::Aliased<Order, OrderAlias>>()
        .select((AliasedCol::<OrderAlias, OrderTotal>::new(),));
    let right = SelectQuery::<MockDialect>::new()
        .from::<Order>()
        .select((OrderTotal,));

    let built = left
        .union(right)
        .order_by_output_asc(AliasedCol::<OrderAlias, OrderTotal>::new())
        .build()
        .expect("valid query");

    // The criterion names the output column, so the alias of the left statement
    // never reaches the ordering of the combined result.
    assert_eq!(
        built.sql,
        "SELECT \"o\".\"total\" FROM \"orders\" AS \"o\" UNION SELECT \"total\" FROM \"orders\" \
         ORDER BY \"total\" ASC"
    );
    assert!(built.params.is_empty());
}

#[test]
fn a_hand_assembled_compound_statement_of_mismatched_arity_is_rejected() {
    // The fluent builders keep this shape out of reach: a combined statement fixes
    // the rows it produces, so `select` cannot rebind them. A statement assembled
    // from AST data can still combine operands of different arity, which no engine
    // accepts, so the mismatch is caught while building.
    let left = SelectQuery::<MockDialect>::new()
        .from::<User>()
        .select((UserId, UserName));
    let right = SelectQuery::<MockDialect>::new()
        .from::<User>()
        .select((UserId,));

    let mut ast = left.into_ast();
    ast.set_operations.push(SetOpAst {
        kind: SetOpKind::Union,
        query: right.into_ast(),
    });

    assert_eq!(
        ast.validate(),
        Err(BuildError::SetOperandArity {
            operand: 0,
            expected: 2,
            actual: 1,
        })
    );
}

#[test]
fn a_compound_ordering_can_fix_where_nulls_sort() {
    fn base() -> SelectQuery<MockDialect, Order, (i64, f64), HasProjection> {
        SelectQuery::<MockDialect>::new()
            .from::<Order>()
            .select((OrderId, OrderTotal))
    }

    let built = base()
        .union(base())
        .order_by_output_desc_nulls_last(OrderTotal)
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "SELECT \"id\", \"total\" FROM \"orders\" UNION SELECT \"id\", \"total\" FROM \"orders\" \
         ORDER BY \"total\" DESC NULLS LAST"
    );
}
