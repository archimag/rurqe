//! Join rendering, joined scopes, and outer-join nullability.

use super::{
    ManagerAlias, MockDialect, Order, OrderId, OrderTotal, OrderUserId, RecentOrder, User, UserId,
};
use crate::expr::{Expr, SqlVal};
use crate::query::*;
use crate::row::FromRow;
use crate::schema::{Aliased, AliasedCol, At, Column, Next, Table, TableInScope};

#[test]
fn inner_join_renders_join_clause_before_filter() {
    let query: SelectQuery<MockDialect, Joined<User, Order>, (i64, i64), HasProjection> =
        SelectQuery::<MockDialect>::new()
            .from::<User>()
            .inner_join(Order, Expr::column::<OrderUserId>().eq(7i64))
            .select((UserId, OrderId));

    let built = query
        .where_(Expr::column::<UserId>().gt(1i64))
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "SELECT \"users\".\"id\", \"orders\".\"id\" FROM \"users\" INNER JOIN \"orders\" \
         ON (\"orders\".\"user_id\" = $1) WHERE (\"users\".\"id\" > $2)"
    );
    assert_eq!(built.params, vec![SqlVal::I64(7), SqlVal::I64(1)]);
}

#[test]
fn joined_scope_tracks_every_table_in_from_order() {
    // A table the query already addresses is joined through an alias marker: two
    // relations of the same name would be rejected by the engine.
    let built = SelectQuery::<MockDialect>::new()
        .from::<User>()
        .inner_join(Order, Expr::column::<OrderUserId>().eq(1i64))
        .inner_join(
            Aliased::<User, ManagerAlias>::new(),
            Expr::column::<UserId>().eq(Expr::column::<AliasedCol<ManagerAlias, UserId>>()),
        )
        .select((OrderId,))
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "SELECT \"orders\".\"id\" FROM \"users\" INNER JOIN \"orders\" \
         ON (\"orders\".\"user_id\" = $1) INNER JOIN \"users\" AS \"m\" \
         ON (\"users\".\"id\" = \"m\".\"id\")"
    );
    assert_eq!(built.params, vec![SqlVal::I64(1)]);
}

#[test]
fn a_table_joined_twice_without_an_alias_is_rejected_before_rendering() {
    // The `ON` predicate and the projection deliberately stay outside the scope
    // of the duplicated relation: a direct column reference to the joined table
    // is already ambiguous between its two scope positions, so this is a
    // compilable form that used to render an invalid duplicate `FROM` / `JOIN`.
    let query = SelectQuery::<MockDialect>::new()
        .from::<User>()
        .inner_join(User, Expr::column::<UserId>().without_scope().eq(1i64))
        .select((Expr::count_all(),));

    assert_eq!(
        query.build(),
        Err(BuildError::DuplicateRelation { name: "users" })
    );
}

#[test]
fn the_same_alias_joined_twice_is_rejected_before_rendering() {
    let query = SelectQuery::<MockDialect>::new()
        .from::<Aliased<User, ManagerAlias>>()
        .inner_join(
            Aliased::<User, ManagerAlias>::new(),
            Expr::column::<AliasedCol<ManagerAlias, UserId>>()
                .without_scope()
                .eq(1i64),
        )
        .select((Expr::count_all(),));

    assert_eq!(
        query.build(),
        Err(BuildError::DuplicateRelation { name: "m" })
    );
}

#[test]
fn the_duplicate_relation_diagnostic_is_pinned_verbatim() {
    assert_eq!(
        BuildError::DuplicateRelation { name: "users" }.to_string(),
        "duplicate unaliased relation `users` in query scope; \
         each occurrence of a table must have a distinct alias via `table_alias!` or `.as_alias(...)`"
    );
}

#[test]
fn single_table_and_joined_scopes_are_query_scopes() {
    fn assert_scope<S: TableScope>() {}

    assert_scope::<User>();
    assert_scope::<Joined<User, Order>>();
    assert_scope::<LeftJoined<User, Order>>();
    assert_scope::<CrossJoined<User, Order>>();
    assert_scope::<FullOuterJoined<User, Order>>();
    assert_scope::<Joined<Joined<User, Order>, User>>();
    assert_scope::<Joined<LeftJoined<User, Order>, User>>();
}

#[test]
fn cross_join_renders_unconditional_cross_join() {
    let built = SelectQuery::<MockDialect>::new()
        .from::<User>()
        .cross_join(Order)
        .select((UserId, OrderId))
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "SELECT \"users\".\"id\", \"orders\".\"id\" FROM \"users\" CROSS JOIN \"orders\""
    );
    assert!(built.params.is_empty());

    let built_value = SelectQuery::<MockDialect>::new()
        .from_table(User)
        .cross_join(Order)
        .select((UserId, OrderId))
        .build()
        .expect("valid query");

    assert_eq!(built_value.sql, built.sql);
}

#[test]
fn full_outer_join_lifts_both_relations_into_options() {
    type FullOuterScope = FullOuterJoined<User, Order>;
    type NullablePair = (Option<i64>, Option<i64>);

    let query: SelectQuery<MockDialect, FullOuterScope, NullablePair, HasProjection> =
        SelectQuery::<MockDialect>::new()
            .from::<User>()
            .full_outer_join(
                Order,
                Expr::column::<UserId>().eq(Expr::column::<OrderUserId>()),
            )
            .select((UserId, OrderId));

    let built = query.build().expect("valid query");

    assert_eq!(
        built.sql,
        "SELECT \"users\".\"id\", \"orders\".\"id\" FROM \"users\" FULL OUTER JOIN \"orders\" \
         ON (\"users\".\"id\" = \"orders\".\"user_id\")"
    );
    assert!(built.params.is_empty());

    let built_value = SelectQuery::<MockDialect>::new()
        .from_table(User)
        .full_outer_join(
            Order,
            Expr::column::<UserId>().eq(Expr::column::<OrderUserId>()),
        )
        .select((UserId, OrderId))
        .build()
        .expect("valid query");

    assert_eq!(built_value.sql, built.sql);
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
        .left_join(Order, Expr::column::<OrderUserId>().eq(7i64))
        .inner_join(
            Aliased::<User, ManagerAlias>::new(),
            Expr::column::<UserId>().eq(Expr::column::<AliasedCol<ManagerAlias, UserId>>()),
        );
    let total = query.nullable::<OrderTotal>();

    let built = query.select((OrderId, total)).build().expect("valid query");

    assert_eq!(
        built.sql,
        "SELECT \"orders\".\"id\", \"orders\".\"total\" FROM \"users\" LEFT JOIN \"orders\" \
         ON (\"orders\".\"user_id\" = $1) INNER JOIN \"users\" AS \"m\" \
         ON (\"users\".\"id\" = \"m\".\"id\")"
    );
    assert_eq!(built.params, vec![SqlVal::I64(7)]);
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
    let query: SelectQuery<
        MockDialect,
        LeftJoined<User, Order>,
        (i64, Option<f64>),
        HasProjection,
    > = SelectQuery::<MockDialect>::new()
        .from::<User>()
        .left_join(Order, Expr::column::<OrderUserId>().eq(7i64))
        .select((UserId, OrderTotal));

    let built = query.build().expect("valid query");

    assert_eq!(
        built.sql,
        "SELECT \"users\".\"id\", \"orders\".\"total\" FROM \"users\" LEFT JOIN \"orders\" \
         ON (\"orders\".\"user_id\" = $1)"
    );
    assert_eq!(built.params, vec![SqlVal::I64(7)]);
}

#[test]
fn inner_joined_columns_keep_their_declared_types() {
    let query: SelectQuery<MockDialect, Joined<User, Order>, (i64, i64, f64), HasProjection> =
        SelectQuery::<MockDialect>::new()
            .from::<User>()
            .inner_join(
                Order,
                Expr::column::<UserId>().eq(Expr::column::<OrderUserId>()),
            )
            .select((UserId, OrderId, OrderTotal));

    assert_eq!(
        query.build().expect("valid query").sql,
        "SELECT \"users\".\"id\", \"orders\".\"id\", \"orders\".\"total\" FROM \"users\" \
         INNER JOIN \"orders\" ON (\"users\".\"id\" = \"orders\".\"user_id\")"
    );
}

#[test]
fn nested_left_join_stays_nullable_inside_an_inner_join() {
    type NestedScope = Joined<LeftJoined<User, Order>, Aliased<User, ManagerAlias>>;

    let query: SelectQuery<MockDialect, NestedScope, (Option<i64>,), HasProjection> =
        SelectQuery::<MockDialect>::new()
            .from::<User>()
            .left_join(Order, Expr::column::<OrderUserId>().eq(1i64))
            .inner_join(
                Aliased::<User, ManagerAlias>::new(),
                Expr::column::<UserId>().eq(Expr::column::<AliasedCol<ManagerAlias, UserId>>()),
            )
            .select((OrderId,));

    assert_eq!(
        query.build().expect("valid query").sql,
        "SELECT \"orders\".\"id\" FROM \"users\" LEFT JOIN \"orders\" \
         ON (\"orders\".\"user_id\" = $1) INNER JOIN \"users\" AS \"m\" \
         ON (\"users\".\"id\" = \"m\".\"id\")"
    );
}

#[test]
fn explicit_nullable_marker_matches_automatic_lifting() {
    fn nullable_scope(
    ) -> SelectQuery<MockDialect, LeftJoined<User, Order>, (i64, Option<f64>), HasProjection> {
        SelectQuery::<MockDialect>::new()
            .from::<User>()
            .left_join(Order, Expr::column::<OrderUserId>().eq(3i64))
            .select((UserId, OrderTotal))
    }

    let automatic = nullable_scope();
    let explicit = {
        let query = SelectQuery::<MockDialect>::new()
            .from::<User>()
            .left_join(Order, Expr::column::<OrderUserId>().eq(3i64));
        let total = query.nullable::<OrderTotal>();
        let query: SelectQuery<
            MockDialect,
            LeftJoined<User, Order>,
            (i64, Option<f64>),
            HasProjection,
        > = query.select((UserId, total));
        query
    };

    assert_eq!(
        automatic.build().expect("valid query"),
        explicit.build().expect("valid query")
    );

    // An already nullable column keeps a single layer of `Option`.
    let single_layer: SelectQuery<
        MockDialect,
        LeftJoined<User, Order>,
        (Option<f64>,),
        HasProjection,
    > = SelectQuery::<MockDialect>::new()
        .from::<User>()
        .left_join(Order, Expr::column::<OrderUserId>().eq(3i64))
        .select((Nullable::<OrderTotal>::new(),));

    assert_eq!(
        single_layer.build().expect("valid query").sql,
        "SELECT \"orders\".\"total\" FROM \"users\" LEFT JOIN \"orders\" \
         ON (\"orders\".\"user_id\" = $1)"
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
fn value_join_arguments_infer_joined_tables() {
    let built = SelectQuery::<MockDialect>::new()
        .from_table(User)
        .inner_join(Order, Expr::column::<OrderUserId>().eq(7i64))
        .select((UserId, OrderTotal))
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "SELECT \"users\".\"id\", \"orders\".\"total\" FROM \"users\" INNER JOIN \"orders\" \
         ON (\"orders\".\"user_id\" = $1)"
    );
    assert_eq!(built.params, vec![SqlVal::I64(7)]);
}

#[test]
fn value_left_join_arguments_lift_columns_into_options() {
    let query: SelectQuery<
        MockDialect,
        LeftJoined<User, Order>,
        (i64, Option<f64>),
        HasProjection,
    > = SelectQuery::<MockDialect>::new()
        .from_table(User)
        .left_join(Order, Expr::column::<OrderUserId>().eq(7i64))
        .select((UserId, OrderTotal));

    assert_eq!(
        query.build().expect("valid query").sql,
        "SELECT \"users\".\"id\", \"orders\".\"total\" FROM \"users\" LEFT JOIN \"orders\" \
         ON (\"orders\".\"user_id\" = $1)"
    );
}

#[test]
fn a_column_marker_renders_the_same_join_predicate_as_an_expression() {
    use crate::expr::Expression;

    type InnerQuery = SelectQuery<MockDialect, Joined<User, Order>, (i64, i64), HasProjection>;

    let shorthand: InnerQuery = SelectQuery::<MockDialect>::new()
        .from::<User>()
        .inner_join(Order, OrderUserId.eq(UserId))
        .select((UserId, OrderId));
    let explicit: InnerQuery = SelectQuery::<MockDialect>::new()
        .from::<User>()
        .inner_join(
            Order,
            Expr::column::<OrderUserId>().eq(Expr::column::<UserId>()),
        )
        .select((UserId, OrderId));

    let shorthand = shorthand.build().expect("valid query");
    let explicit = explicit.build().expect("valid query");

    assert_eq!(shorthand.sql, explicit.sql);
    assert_eq!(shorthand.params, explicit.params);
    assert_eq!(
        shorthand.sql,
        "SELECT \"users\".\"id\", \"orders\".\"id\" FROM \"users\" INNER JOIN \"orders\" ON (\"orders\".\"user_id\" = \"users\".\"id\")"
    );
    assert!(shorthand.params.is_empty());

    type LeftQuery =
        SelectQuery<MockDialect, LeftJoined<User, Order>, (i64, Option<i64>), HasProjection>;

    let shorthand: LeftQuery = SelectQuery::<MockDialect>::new()
        .from::<User>()
        .left_join(Order, OrderUserId.eq(UserId))
        .select((UserId, OrderId));
    let explicit: LeftQuery = SelectQuery::<MockDialect>::new()
        .from::<User>()
        .left_join(
            Order,
            Expr::column::<OrderUserId>().eq(Expr::column::<UserId>()),
        )
        .select((UserId, OrderId));

    let shorthand = shorthand.build().expect("valid query");
    let explicit = explicit.build().expect("valid query");

    assert_eq!(shorthand.sql, explicit.sql);
    assert_eq!(shorthand.params, explicit.params);
    assert_eq!(
        shorthand.sql,
        "SELECT \"users\".\"id\", \"orders\".\"id\" FROM \"users\" LEFT JOIN \"orders\" ON (\"orders\".\"user_id\" = \"users\".\"id\")"
    );
    assert!(shorthand.params.is_empty());
}
