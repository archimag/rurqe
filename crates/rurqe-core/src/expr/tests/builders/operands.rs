//! Tests for the markers a comparison derives and for column markers as operands.

use super::*;

#[test]
fn a_comparison_derives_its_marker_from_both_operands() {
    // An aggregate compared with a bound value still collapses the rows of its
    // group, so the predicate keeps the aggregate marker and a grouped query
    // accepts it in `HAVING` or in its projection list.
    let _: Expr<bool, _, Aggregate> = sum(OrderTotal).gt(1.0f64);
    let _: Expr<bool, _, Aggregate> = count_all().eq(0i64);
    let _: Expr<bool, _, Aggregate> = count_distinct(OrderQty).lt(count_distinct(OrderQty));

    // The same aggregate compared with a bare column reads one value per row of the
    // group, so the predicate is per-row and the grouped rules reject it at the call
    // site instead of the engine rejecting the rendered statement.
    let _: Expr<bool, _, NonAggregate> = sum(OrderTotal).gt(Expr::column::<OrderTax>());

    // A range folds the markers of the tested expression and both of its bounds.
    let _: Expr<bool, _, Aggregate> = sum(OrderTotal).between(0.0f64, 1.0f64);
    let _: Expr<bool, _, NonAggregate> =
        sum(OrderTotal).between(0.0f64, Expr::column::<OrderTax>());

    // A membership list folds the markers of its candidates the same way.
    let _: Expr<bool, _, Aggregate> = sum(OrderTotal).in_list(vec![0.0f64, 1.0f64]);
    let _: Expr<bool, _, NonAggregate> = sum(OrderTotal).in_list(vec![Expr::column::<OrderTax>()]);

    // Combining two predicates folds their markers as well.
    let _: Expr<bool, _, Aggregate> = sum(OrderTotal).gt(1.0f64).and(sum(OrderTax).lt(1.0f64));
}

#[test]
fn the_aggregate_clauses_stay_on_the_call_that_recorded_them() {
    // `DISTINCT` and `FILTER (WHERE ...)` restrict the rows an aggregate call
    // aggregates, so both are offered by the call itself, and a call that already
    // carries a filter keeps accepting further conditions.
    let call: Expr<f64, Order, DirectAggregate> = sum(OrderTotal);
    let distinct: Expr<f64, Order, DirectAggregate> = call.distinct();
    assert!(matches!(
        distinct.kind(),
        ExprKind::Aggregate { distinct: true, .. }
    ));

    let filtered: Expr<i64, (Order, Order), FilteredAggregate> =
        count_distinct(OrderQty).filter_where(OrderQty.gt(0i64));
    let accumulated: Expr<i64, _, FilteredAggregate> = filtered.filter_where(OrderQty.lt(100i64));
    assert!(matches!(
        accumulated.kind(),
        ExprKind::Aggregate {
            filter: Some(_),
            ..
        }
    ));
}

#[test]
fn the_valid_aggregate_forms_render_verbatim() {
    // The clauses keep rendering where SQL puts them: the modifier inside the call
    // and the filter after it, so the marker rules changed no rendered byte.
    let (sql, params) = render(count_distinct(OrderTotal).kind());
    assert_eq!(sql, "COUNT(DISTINCT \"total\")");
    assert!(params.is_empty());

    let (sql, params) = render(
        count_distinct(OrderQty)
            .filter_where(OrderQty.gt(0i64))
            .kind(),
    );
    assert_eq!(sql, "COUNT(DISTINCT \"qty\") FILTER (WHERE (\"qty\" > ?))");
    assert_eq!(params, vec![SqlVal::I64(0)]);

    // A predicate computed from an aggregate and a bound value renders unchanged as
    // well, so the grouped statements built from it keep their SQL.
    let (sql, params) = render(sum(OrderTotal).gt(1.0f64).kind());
    assert_eq!(sql, "(SUM(\"total\") > ?)");
    assert_eq!(params, vec![SqlVal::F64(1.0)]);

    // The per-row predicate the grouped rules reject is still a renderable
    // expression: the rejection happens while the statement is checked, not in the
    // renderer.
    let (sql, params) = render(sum(OrderTotal).gt(Expr::column::<OrderTax>()).kind());
    assert_eq!(sql, "(SUM(\"total\") > \"tax\")");
    assert!(params.is_empty());
}

#[test]
fn a_column_marker_is_a_right_hand_operand_of_every_comparison() {
    crate::table!(
        Order => "orders" {
            OrderId => "id": i64,
            OrderUserId => "user_id": i64,
        }
    );

    // The marker converts into the very node `Expr::column::<C>()` builds, and
    // the merged scope claims the tables of both operands, so the shorthand and
    // the explicit form produce the same expression.
    let equal: Expr<bool, (Order, User), NonAggregate, ()> = OrderUserId.eq(UserId);
    assert_eq!(
        equal,
        Expr::column::<OrderUserId>().eq(Expr::column::<UserId>())
    );
    assert_eq!(
        equal.kind(),
        &ExprKind::Binary {
            op: BinaryOp::Eq,
            lhs: Box::new(ExprKind::Column {
                table: "orders",
                name: "user_id",
            }),
            rhs: Box::new(ExprKind::Column {
                table: "users",
                name: "id",
            }),
        }
    );

    let distinct: Expr<bool, (Order, User), NonAggregate, ()> = OrderUserId.ne(UserId);
    assert_eq!(
        distinct,
        Expr::column::<OrderUserId>().ne(Expr::column::<UserId>())
    );

    let above: Expr<bool, (Order, User), NonAggregate, ()> = OrderUserId.gt(UserId);
    assert_eq!(
        above,
        Expr::column::<OrderUserId>().gt(Expr::column::<UserId>())
    );

    let below: Expr<bool, (Order, User), NonAggregate, ()> = OrderUserId.lt(UserId);
    assert_eq!(
        below,
        Expr::column::<OrderUserId>().lt(Expr::column::<UserId>())
    );
}

#[test]
fn a_column_marker_is_a_between_bound_and_a_membership_candidate() {
    crate::table!(
        Order => "orders" {
            OrderId => "id": i64,
            OrderUserId => "user_id": i64,
        }
    );

    // Bounds written as markers claim their table just like the tested column
    // does, so the range references all three relations of the shown form.
    let range: Expr<bool, (User, Order, Order), NonAggregate, ()> =
        UserId.between(OrderId, OrderUserId);
    assert_eq!(
        range,
        Expr::column::<UserId>().between(Expr::column::<OrderId>(), Expr::column::<OrderUserId>())
    );
    assert!(matches!(range.kind(), ExprKind::Between { .. }));

    let members: Expr<bool, (User, Order), NonAggregate, ()> = UserId.in_list(vec![OrderUserId]);
    assert_eq!(
        members,
        Expr::column::<UserId>().in_list(vec![Expr::column::<OrderUserId>()])
    );
    assert!(matches!(
        members.kind(),
        ExprKind::InList { negated: false, .. }
    ));

    let absent: Expr<bool, (User, Order), NonAggregate, ()> = UserId.not_in_list(vec![OrderId]);
    assert_eq!(
        absent,
        Expr::column::<UserId>().not_in_list(vec![Expr::column::<OrderId>()])
    );
    assert!(matches!(
        absent.kind(),
        ExprKind::InList { negated: true, .. }
    ));
}

#[test]
fn a_column_marker_folds_the_aggregation_marker_of_the_compared_aggregate() {
    // A bound operand is the same in every row of the group, so the predicate
    // built from an aggregate keeps collapsing it.
    let over_a_value: Expr<bool, Order, Aggregate, ()> = sum(OrderTotal).gt(0.0f64);
    assert_eq!(
        over_a_value.kind(),
        &ExprKind::Binary {
            op: BinaryOp::Gt,
            lhs: Box::new(ExprKind::Aggregate {
                func: AggregateFunc::Sum,
                distinct: false,
                arg: Some(Box::new(ExprKind::Column {
                    table: "orders",
                    name: "total",
                })),
                filter: None,
            }),
            rhs: Box::new(ExprKind::Value(SqlVal::F64(0.0))),
        }
    );

    // A marker reads a column of the group, so the same aggregate compared with
    // it computes one value per row and the predicate is per-row.
    let over_a_column: Expr<bool, (Order, Order), NonAggregate, ()> = sum(OrderTotal).gt(OrderTax);
    assert_eq!(
        over_a_column.kind(),
        &ExprKind::Binary {
            op: BinaryOp::Gt,
            lhs: Box::new(ExprKind::Aggregate {
                func: AggregateFunc::Sum,
                distinct: false,
                arg: Some(Box::new(ExprKind::Column {
                    table: "orders",
                    name: "total",
                })),
                filter: None,
            }),
            rhs: Box::new(ExprKind::Column {
                table: "orders",
                name: "tax",
            }),
        }
    );
}
