//! Tests for arithmetic composition and the aggregation markers it folds.

use super::*;

#[test]
fn arithmetic_operators_build_binary_expressions() {
    let raised: Expr<f64, Order> = Expr::column::<OrderTotal>() + 1.5f64;
    assert_eq!(
        raised.kind(),
        &ExprKind::Binary {
            op: BinaryOp::Add,
            lhs: Box::new(ExprKind::Column {
                table: "orders",
                name: "total",
            }),
            rhs: Box::new(ExprKind::Value(SqlVal::F64(1.5))),
        }
    );

    let net: Expr<f64, (Order, Order)> = Expr::column::<OrderTotal>() - OrderTax;
    assert_eq!(
        net.kind(),
        &ExprKind::Binary {
            op: BinaryOp::Sub,
            lhs: Box::new(ExprKind::Column {
                table: "orders",
                name: "total",
            }),
            rhs: Box::new(ExprKind::Column {
                table: "orders",
                name: "tax",
            }),
        }
    );

    let doubled: Expr<i64, Order> = Expr::column::<OrderQty>() * 2i64;
    assert!(matches!(
        doubled.kind(),
        ExprKind::Binary {
            op: BinaryOp::Mul,
            ..
        }
    ));

    let halved: Expr<f64, Order> = Expr::column::<OrderTotal>() / 2.0f64;
    assert!(matches!(
        halved.kind(),
        ExprKind::Binary {
            op: BinaryOp::Div,
            ..
        }
    ));

    let parity: Expr<i64, Order> = Expr::column::<OrderQty>() % 2i64;
    assert!(matches!(
        parity.kind(),
        ExprKind::Binary {
            op: BinaryOp::Rem,
            ..
        }
    ));
}

#[test]
fn arithmetic_merges_the_scopes_of_its_operands() {
    // A bound value references no table, so it adds nothing to the scope.
    let _: Expr<f64, Order> = Expr::column::<OrderTotal>() + 0.5f64;

    // Two columns of the same table reference it twice, exactly as they do when
    // they are compared with each other.
    let _: Expr<f64, (Order, Order)> = Expr::column::<OrderTotal>() + OrderTax;

    // A column of another table extends the scope with that table.
    let _: Expr<i64, (Order, User)> = Expr::column::<OrderQty>() + UserId;
}

#[test]
fn arithmetic_preserves_the_aggregation_marker() {
    // The aggregate references `orders`, and the bound operand adds no table, so
    // the arithmetic still collapses the group; combining the call with an
    // operand makes the expression an `Aggregate` instead of the call itself.
    let adjusted: Expr<f64, Order, Aggregate> = sum(OrderTotal) + 1.0f64;

    assert!(matches!(
        adjusted.kind(),
        ExprKind::Binary {
            op: BinaryOp::Add,
            ..
        }
    ));
}

#[test]
fn arithmetic_degrades_a_filtered_aggregate_next_to_a_constant() {
    let filtered = sum(OrderTotal).filter_where(OrderQty.gt(0i64));

    // A constant operand is the same in every row of the group, so the scaled
    // filtered aggregate keeps collapsing its group and stays projectable from
    // a grouped query. The combination is no longer the call itself, though, so
    // it degrades to the general `Aggregate` marker and takes no `FILTER` clause
    // of its own.
    let scaled: Expr<f64, _, Aggregate> = filtered * 2.0f64;
    assert!(matches!(
        scaled.kind(),
        ExprKind::Binary {
            op: BinaryOp::Mul,
            ..
        }
    ));

    // An operand reading a column computes one value per row, so the arithmetic
    // stops collapsing the group.
    let untracked: Expr<f64, _, NonAggregate> =
        sum(OrderTotal).filter_where(OrderQty.gt(0i64)) + OrderTax;
    assert!(matches!(
        untracked.kind(),
        ExprKind::Binary {
            op: BinaryOp::Add,
            ..
        }
    ));
}

#[test]
fn arithmetic_renders_its_operator_between_the_operands() {
    let net = Expr::column::<OrderTotal>() - OrderTax;

    assert_eq!(render(net.kind()).0, "(\"total\" - \"tax\")");
}

#[test]
fn an_operand_reading_a_column_drops_the_aggregation_marker() {
    // The arithmetic reads `tax` once per row, so it no longer collapses the
    // group even though its left operand is an aggregate.
    let folded: Expr<f64, (Order, Order), NonAggregate> = sum(OrderTotal) + OrderTax;

    // The aggregate reads `total` and the constant adds no table, so the
    // arithmetic stays projectable from a grouped query.
    let scaled: Expr<f64, Order, Aggregate> = sum(OrderTotal) * 2.0f64;

    // Two per-row operands compute one value per row, whether they read a column
    // or a value.
    let per_row: Expr<f64, (Order, Order), NonAggregate> = Expr::column::<OrderTotal>() + OrderTax;

    assert!(matches!(
        folded.kind(),
        ExprKind::Binary {
            op: BinaryOp::Add,
            ..
        }
    ));
    assert!(matches!(
        scaled.kind(),
        ExprKind::Binary {
            op: BinaryOp::Mul,
            ..
        }
    ));
    assert!(matches!(
        per_row.kind(),
        ExprKind::Binary {
            op: BinaryOp::Add,
            ..
        }
    ));
}

#[test]
fn a_case_over_aggregates_stays_an_aggregate() {
    // Every operand of the CASE is an aggregate or a constant, so the folding
    // algebra of the builder keeps the finished expression an aggregate, and a
    // grouped query accepts it as a projection.
    let tiered: Expr<f64, (Order, Order), Aggregate> = Expr::<f64>::case()
        .when(sum(OrderTotal).gt(10.0f64), sum(OrderTotal))
        .else_(0.0f64)
        .end();

    assert!(matches!(
        tiered.kind(),
        ExprKind::Case { branches, .. } if matches!(
            branches[0].condition,
            ExprKind::Binary { op: BinaryOp::Gt, .. }
        )
    ));

    // A per-row operand stops the expression collapsing the group, so the
    // finished CASE is rejected by a grouped query unless its column is grouped.
    let per_row: Expr<f64, (Order, Order), NonAggregate> = Expr::<f64>::case()
        .when(sum(OrderTotal).gt(10.0f64), OrderTotal)
        .else_(0.0f64)
        .end();

    assert!(matches!(per_row.kind(), ExprKind::Case { .. }));
}

#[test]
fn a_comparison_never_becomes_the_aggregate_call_it_is_built_from() {
    // The comparison folds the markers of both operands, so it collapses the rows
    // of its group without being the aggregate call itself: `DISTINCT` and
    // `FILTER (WHERE ...)` attach to a call, and the predicate is no call at all.
    let over_two_calls: Expr<bool, _, Aggregate> = sum(OrderTotal).gt(sum(OrderTax));
    assert!(matches!(
        over_two_calls.kind(),
        ExprKind::Binary {
            op: BinaryOp::Gt,
            ..
        }
    ));

    // A bound operand references no table, so it leaves the marker of the call
    // standing while the comparison still is no call of its own.
    let over_a_value: Expr<bool, _, Aggregate> = count_all().gt(1i64);
    assert!(matches!(
        over_a_value.kind(),
        ExprKind::Binary {
            op: BinaryOp::Gt,
            ..
        }
    ));
}

#[test]
fn a_bare_direct_aggregate_renders_without_modifiers() {
    let (sql, params) = render(sum(OrderTotal).kind());

    assert_eq!(sql, "SUM(\"total\")");
    assert!(params.is_empty());
}

#[test]
fn distinct_and_filter_stay_on_the_call_that_recorded_them() {
    // Both modifiers belong to the aggregate call itself, so the rendered
    // function carries them instead of losing them on the way into the
    // statement the call is projected from.
    let counted = count_distinct(OrderQty).filter_where(OrderQty.gt(0i64));
    let (sql, params) = render(counted.kind());
    assert_eq!(sql, "COUNT(DISTINCT \"qty\") FILTER (WHERE (\"qty\" > ?))");
    assert_eq!(params, vec![SqlVal::I64(0)]);

    let summed = sum_distinct(OrderTotal).filter_where(OrderTax.gt(0.0f64));
    let (sql, params) = render(summed.kind());
    assert_eq!(sql, "SUM(DISTINCT \"total\") FILTER (WHERE (\"tax\" > ?))");
    assert_eq!(params, vec![SqlVal::F64(0.0)]);
}

#[test]
fn arithmetic_preserves_the_filter_of_a_direct_aggregate() {
    // The filter is recorded in the aggregate node itself, so combining the call
    // with a constant keeps the clause in the rendered statement instead of
    // dropping it silently.
    let scaled = sum(OrderTotal).filter_where(OrderQty.gt(0i64)) * 2.0f64;

    let (sql, params) = render(scaled.kind());

    assert_eq!(sql, "(SUM(\"total\") FILTER (WHERE (\"qty\" > ?)) * ?)");
    assert_eq!(params, vec![SqlVal::I64(0), SqlVal::F64(2.0)]);
}

#[test]
fn the_modifier_markers_separate_the_call_from_a_composite_expression() {
    // The call carries the marker that gates `distinct` and `filter_where` ...
    let call: Expr<f64, Order, DirectAggregate> = sum(OrderTotal);
    // ... a filtered call keeps a marker of its own ...
    let filtered: Expr<f64, _, FilteredAggregate> = call.filter_where(OrderQty.gt(0i64));
    // ... while combining either call degrades the expression to the general
    // grouping marker: it still collapses its group, but offers no modifier.
    let scaled_filtered: Expr<f64, _, Aggregate> = filtered * 2.0f64;
    let scaled_call: Expr<f64, _, Aggregate> = sum(OrderTotal) * 2.0f64;

    assert!(matches!(
        scaled_filtered.kind(),
        ExprKind::Binary {
            op: BinaryOp::Mul,
            ..
        }
    ));
    assert!(matches!(
        scaled_call.kind(),
        ExprKind::Binary {
            op: BinaryOp::Mul,
            ..
        }
    ));
}

#[test]
fn compositions_fold_aggregate_calls_into_the_general_marker() {
    // Whatever markers the operands carry, a combination is never the aggregate
    // call itself, so every composition written over aggregates — arithmetic, a
    // scalar function, a `CASE` branch — folds its operands into the general
    // `Aggregate` marker and keeps `distinct` and `filter_where` off the
    // produced tree.
    fn assert_general<T, Scope, D>(_: Expr<T, Scope, Aggregate, D>) {}

    // A composition consumes the expressions it folds, so every assertion
    // builds its operands freshly instead of sharing one moved binding. The
    // return types pin the markers and the merged scope of each call as well.
    fn filtered() -> Expr<f64, (Order, Order), FilteredAggregate> {
        sum(OrderTotal).filter_where(OrderQty.gt(0i64))
    }

    fn distinct() -> Expr<f64, Order, DirectAggregate> {
        sum_distinct(OrderTotal)
    }

    // Arithmetic pairs cover the aggregate markers on either side of the
    // operator, including the already-combined general one.
    assert_general(sum(OrderTotal) + sum(OrderTax));
    assert_general(sum(OrderTotal) + filtered());
    assert_general(distinct() + filtered());
    assert_general(filtered() + distinct());
    assert_general(filtered() + filtered());
    assert_general((sum(OrderTotal) + 1.0f64) + filtered());

    // Scalar functions fold their aggregate arguments the same way.
    assert_general(coalesce(filtered(), 0.0f64));
    assert_general(nullif(filtered(), sum(OrderTax)));

    // CASE branches fold the markers of their conditions and results as well.
    assert_general(
        Expr::<f64>::case()
            .when(filtered().gt(0.0f64), sum(OrderTotal))
            .else_(0.0f64)
            .end(),
    );
}

#[test]
fn a_forged_direct_aggregate_marker_reports_the_dropped_filter_in_every_profile() {
    // `Expr::new` stays crate-internal, so only this crate can assemble a tree
    // that disagrees with the aggregation marker it carries. The branch handling
    // that contradiction is checked without relying on an assertion, so the
    // regression is caught in a release build as well: the returned tree carries
    // a placeholder naming the discarded clause instead of dropping it silently
    // while the return type keeps advertising the `FILTER` clause.
    let forged: Expr<i64, Order, DirectAggregate> = Expr::new(ExprKind::Column {
        table: "orders",
        name: "qty",
    });

    let marked = forged.filter_where(OrderQty.gt(0i64));

    let ExprKind::Raw {
        fragments,
        params: bound,
    } = marked.kind()
    else {
        panic!(
            "expected the diagnostic placeholder, got {:?}",
            marked.kind()
        );
    };
    assert_eq!(
        fragments.len(),
        1,
        "the placeholder is a single fragment carrying no parameter"
    );
    assert!(
        fragments.first().is_some_and(|fragment| fragment.contains("FILTER")),
        "the placeholder names the clause it replaced: {fragments:?}"
    );
    assert!(bound.is_empty());

    // The placeholder reaches the rendered statement, so the discarded predicate
    // surfaces in the SQL a caller inspects instead of vanishing in the build.
    let (sql, rendered_params) = render(marked.kind());
    assert_eq!(sql, fragments.first().unwrap_or_default());
    assert!(rendered_params.is_empty());
}

#[test]
fn a_forged_filtered_aggregate_marker_reports_the_dropped_distinct_in_every_profile() {
    // `Expr::new` stays crate-internal, so only this crate can assemble a tree
    // that disagrees with the aggregation marker it carries. The branch
    // handling that contradiction is checked without relying on an assertion,
    // so the regression is caught in a release build as well: the returned
    // tree carries a placeholder naming the dropped modifier instead of
    // silently mutating nothing while the return type keeps advertising
    // `DISTINCT`.
    let forged: Expr<i64, Order, FilteredAggregate> = Expr::new(ExprKind::Column {
        table: "orders",
        name: "qty",
    });

    let marked = forged.distinct();

    let ExprKind::Raw {
        fragments,
        params: bound,
    } = marked.kind()
    else {
        panic!(
            "expected the diagnostic placeholder, got {:?}",
            marked.kind()
        );
    };
    assert_eq!(
        fragments.len(),
        1,
        "the placeholder is a single fragment carrying no parameter"
    );
    assert!(
        fragments.first().is_some_and(|fragment| fragment.contains("DISTINCT")),
        "the placeholder names the modifier it dropped: {fragments:?}"
    );
    assert!(bound.is_empty());

    // The placeholder reaches the rendered statement, so the dropped modifier
    // surfaces in the SQL a caller inspects instead of vanishing in the build.
    let (sql, rendered_params) = render(marked.kind());
    assert_eq!(sql, fragments.first().unwrap_or_default());
    assert!(rendered_params.is_empty());
}
