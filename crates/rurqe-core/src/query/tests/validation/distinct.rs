//! `DISTINCT` and `DISTINCT ON` ordering rules.
//!
//! `DISTINCT` requires every ordering expression to appear in the projection
//! list, and `DISTINCT ON` requires the leading ordering criteria to repeat the
//! deduplicating expressions. Both rules are checked before a statement is
//! rendered, so the tests below assemble the clauses on the statement data as
//! well as through the fluent builders.

use super::super::{MockDialect, Order, OrderAlias, OrderId, OrderTotal, OrderUserId};
use super::{count_star, empty_ast, relation_statement};
use crate::expr::{ExprKind, OrderCriterion, OrderDirection, SelectAst, Subquery, SubqueryOp};
use crate::query::*;
use crate::schema::{AliasedCol, TableRef};

#[test]
fn distinct_on_qualifies_aliased_columns() {
    // PostgreSQL requires the leading ORDER BY criteria to repeat the
    // DISTINCT ON expressions, so the ordering begins with the user and only
    // then breaks ties by the total.
    let built = SelectQuery::<MockDialect>::new()
        .from::<crate::schema::Aliased<Order, OrderAlias>>()
        .select((
            AliasedCol::<OrderAlias, OrderId>::new(),
            AliasedCol::<OrderAlias, OrderTotal>::new(),
        ))
        .distinct_on(AliasedCol::<OrderAlias, OrderUserId>::new())
        .order_by_asc(AliasedCol::<OrderAlias, OrderUserId>::new())
        .order_by_asc(AliasedCol::<OrderAlias, OrderTotal>::new())
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "SELECT DISTINCT ON (\"o\".\"user_id\") \"o\".\"id\", \"o\".\"total\" \
         FROM \"orders\" AS \"o\" ORDER BY \"o\".\"user_id\" ASC, \"o\".\"total\" ASC"
    );
}

#[test]
fn an_order_by_matching_the_distinct_on_prefix_is_accepted() {
    let built = SelectQuery::<MockDialect>::new()
        .from::<Order>()
        .select((OrderId,))
        .distinct_on(OrderUserId)
        .distinct_on(OrderId)
        .order_by_desc(OrderUserId)
        .order_by_asc(OrderId)
        .order_by_desc(OrderTotal)
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "SELECT DISTINCT ON (\"user_id\", \"id\") \"id\" FROM \"orders\" \
         ORDER BY \"user_id\" DESC, \"id\" ASC, \"total\" DESC"
    );
}

#[test]
fn an_order_by_shorter_than_distinct_on_is_rejected_before_rendering() {
    let query = SelectQuery::<MockDialect>::new()
        .from::<Order>()
        .select((OrderId,))
        .distinct_on(OrderUserId)
        .distinct_on(OrderId)
        .order_by_asc(OrderUserId);

    assert_eq!(
        query.build(),
        Err(BuildError::DistinctOnOrderMismatch { position: 1 })
    );
}

#[test]
fn a_mismatched_leading_order_by_criterion_is_rejected_before_rendering() {
    let query = SelectQuery::<MockDialect>::new()
        .from::<Order>()
        .select((OrderId,))
        .distinct_on(OrderUserId)
        .order_by_asc(OrderTotal)
        .order_by_asc(OrderUserId);

    assert_eq!(
        query.build(),
        Err(BuildError::DistinctOnOrderMismatch { position: 0 })
    );
}

#[test]
fn distinct_on_without_order_by_stays_valid() {
    let built = SelectQuery::<MockDialect>::new()
        .from::<Order>()
        .select((OrderId,))
        .distinct_on(OrderUserId)
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "SELECT DISTINCT ON (\"user_id\") \"id\" FROM \"orders\""
    );
}

#[test]
fn build_error_display_distinct_on_mismatch() {
    let err = BuildError::DistinctOnOrderMismatch { position: 1 };
    let msg = err.to_string();
    assert!(msg.contains("ORDER BY criterion at position 1"));
    assert!(msg.contains("DISTINCT ON"));
}

#[test]
fn build_error_display_distinct_order_not_projected() {
    let err = BuildError::DistinctOrderNotProjected { position: 1 };
    let msg = err.to_string();
    assert!(msg.contains("ORDER BY criterion at position 1"));
    assert!(msg.contains("must appear in the select list"));
}

#[test]
fn build_error_display_distinct_with_distinct_on() {
    let msg = BuildError::DistinctWithDistinctOn.to_string();
    assert!(msg.contains("a statement requests both `DISTINCT` and `DISTINCT ON (...)`"));
    assert!(msg.contains("keep one of the two clauses"));
}

#[test]
fn distinct_then_distinct_on_is_rejected_before_rendering() {
    // Both calls leave the query in the same deduplicated state, so the chain
    // compiles; the pair is rejected while the statement is built instead of
    // rendering the `DISTINCT ON (...)` prefix alone and dropping the
    // whole-row deduplication without a diagnostic.
    let query = SelectQuery::<MockDialect>::new()
        .from::<Order>()
        .select((OrderId,))
        .distinct()
        .distinct_on(OrderUserId)
        .order_by_asc(OrderUserId);

    assert_eq!(query.build(), Err(BuildError::DistinctWithDistinctOn));
}

#[test]
fn a_statement_recording_both_deduplication_flavours_is_rejected() {
    let mut ast = empty_ast();
    ast.from = Some(TableRef::plain("orders"));
    ast.distinct = true;
    ast.projections.push(ExprKind::Column {
        table: "orders",
        name: "id",
    });
    ast.distinct_on.push(ExprKind::Column {
        table: "orders",
        name: "user_id",
    });
    ast.order_by.push(OrderCriterion::new(
        ExprKind::Column {
            table: "orders",
            name: "user_id",
        },
        OrderDirection::Asc,
    ));

    assert_eq!(ast.validate(), Err(BuildError::DistinctWithDistinctOn));
}

#[test]
fn a_statement_recording_both_flavours_reports_them_before_the_lock() {
    // The flavour contradiction names the two clauses the caller has to choose
    // between, while the locking rule only forbids both of them, so the pair
    // is reported first.
    let mut ast = empty_ast();
    ast.from = Some(TableRef::plain("orders"));
    ast.distinct = true;
    ast.projections.push(ExprKind::Column {
        table: "orders",
        name: "user_id",
    });
    ast.distinct_on.push(ExprKind::Column {
        table: "orders",
        name: "user_id",
    });
    ast.order_by.push(OrderCriterion::new(
        ExprKind::Column {
            table: "orders",
            name: "user_id",
        },
        OrderDirection::Asc,
    ));
    ast.lock = Some(crate::expr::LockClause::new(crate::expr::LockStrength::Update));

    assert_eq!(ast.validate(), Err(BuildError::DistinctWithDistinctOn));
}
#[test]
fn a_distinct_statement_ordering_a_projected_expression_is_accepted() {
    let mut ast = empty_ast();
    ast.from = Some(TableRef::plain("orders"));
    ast.distinct = true;
    ast.projections.push(ExprKind::Column {
        table: "orders",
        name: "id",
    });
    ast.order_by.push(OrderCriterion::new(
        ExprKind::Column {
            table: "orders",
            name: "id",
        },
        OrderDirection::Asc,
    ));

    assert_eq!(ast.validate(), Ok(()));
}

#[test]
fn a_distinct_statement_ordering_an_unprojected_expression_is_rejected() {
    let mut ast = empty_ast();
    ast.distinct = true;
    ast.projections.push(ExprKind::Column {
        table: "orders",
        name: "id",
    });
    ast.order_by.push(OrderCriterion::new(
        ExprKind::Column {
            table: "orders",
            name: "total",
        },
        OrderDirection::Asc,
    ));

    assert_eq!(
        ast.validate(),
        Err(BuildError::DistinctOrderNotProjected { position: 0 })
    );
}

#[test]
fn a_distinct_ordering_matches_an_aliased_projection_and_a_bare_position() {
    // The ordering criterion matches the expression the aliased projection
    // wraps, and a bare 1-based position addresses the first projection.
    let mut ast = empty_ast();
    ast.from = Some(TableRef::plain("orders"));
    ast.distinct = true;
    ast.projections.push(ExprKind::Aliased {
        expr: Box::new(ExprKind::Column {
            table: "orders",
            name: "total",
        }),
        alias: "net_total",
    });
    ast.order_by.push(OrderCriterion::new(
        ExprKind::Column {
            table: "orders",
            name: "total",
        },
        OrderDirection::Desc,
    ));
    ast.order_by.push(OrderCriterion::new(
        ExprKind::Raw {
            fragments: vec!["1".to_owned()].into(),
            params: Vec::new(),
        },
        OrderDirection::Asc,
    ));

    assert_eq!(ast.validate(), Ok(()));
}

#[test]
fn a_distinct_ordering_matches_a_column_addressed_through_the_other_node_shape() {
    // A projection built from a schema marker and an ordering built through a
    // table alias name the same column of the same relation, so both shapes
    // resolve to one canonical reference. The statement declares the relation
    // both shapes name, so the qualifier of the ordering resolves against it.
    let mut ast = empty_ast();
    ast.from = Some(TableRef::plain("orders"));
    ast.distinct = true;
    ast.projections.push(ExprKind::Column {
        table: "orders",
        name: "total",
    });
    ast.order_by.push(OrderCriterion::new(
        ExprKind::QualifiedColumn {
            table: "orders",
            column: "total",
        },
        OrderDirection::Asc,
    ));

    assert_eq!(ast.validate(), Ok(()));
}

#[test]
fn a_distinct_ordering_matches_an_aliased_computed_projection() {
    // The projection is exposed under an alias and computes a `CAST` over a
    // column marker, while the ordering names the same computation over the
    // alias-qualified shape of that column. The statement declares the relation
    // both shapes name, so the qualifier of the ordering resolves against it.
    let mut ast = empty_ast();
    ast.from = Some(TableRef::plain("orders"));
    ast.distinct = true;
    ast.projections.push(ExprKind::Aliased {
        expr: Box::new(ExprKind::Cast {
            operand: Box::new(ExprKind::Column {
                table: "orders",
                name: "total",
            }),
            target: "numeric",
        }),
        alias: "net_total",
    });
    ast.order_by.push(OrderCriterion::new(
        ExprKind::Cast {
            operand: Box::new(ExprKind::QualifiedColumn {
                table: "orders",
                column: "total",
            }),
            target: "numeric",
        },
        OrderDirection::Desc,
    ));

    assert_eq!(ast.validate(), Ok(()));
}

#[test]
fn a_distinct_ordering_of_another_computation_is_rejected() {
    // The projection is computed as numeric while the ordering computes text,
    // so the two expressions differ even though both read the same column.
    let mut ast = empty_ast();
    ast.distinct = true;
    ast.projections.push(ExprKind::Cast {
        operand: Box::new(ExprKind::Column {
            table: "orders",
            name: "total",
        }),
        target: "numeric",
    });
    ast.order_by.push(OrderCriterion::new(
        ExprKind::Cast {
            operand: Box::new(ExprKind::QualifiedColumn {
                table: "orders",
                column: "total",
            }),
            target: "text",
        },
        OrderDirection::Asc,
    ));

    assert_eq!(
        ast.validate(),
        Err(BuildError::DistinctOrderNotProjected { position: 0 })
    );
}

#[test]
fn a_distinct_on_prefix_matches_an_order_by_written_through_the_other_node_shape() {
    // `DISTINCT ON` spells the column as an alias-qualified reference while the
    // leading ordering criterion reads the schema-marker shape of the same column
    // of the same relation. Both name one value, so the prefix alignment holds and
    // the statement built past the fluent builders stays valid.
    let mut ast = empty_ast();
    ast.from = Some(TableRef::plain("orders"));
    ast.projections.push(ExprKind::Column {
        table: "orders",
        name: "user_id",
    });
    ast.distinct_on.push(ExprKind::QualifiedColumn {
        table: "orders",
        column: "user_id",
    });
    ast.order_by.push(OrderCriterion::new(
        ExprKind::Column {
            table: "orders",
            name: "user_id",
        },
        OrderDirection::Asc,
    ));

    assert_eq!(ast.validate(), Ok(()));
}

#[test]
fn a_distinct_on_prefix_naming_another_column_is_still_rejected() {
    // The canonical comparison normalises the two spellings of one column; it does
    // not make distinct columns match, so the alignment rule still rejects a leading
    // ordering criterion naming a column the clause does not deduplicate by.
    let mut ast = empty_ast();
    ast.from = Some(TableRef::plain("orders"));
    ast.projections.push(ExprKind::Column {
        table: "orders",
        name: "user_id",
    });
    ast.distinct_on.push(ExprKind::QualifiedColumn {
        table: "orders",
        column: "user_id",
    });
    ast.order_by.push(OrderCriterion::new(
        ExprKind::Column {
            table: "orders",
            name: "total",
        },
        OrderDirection::Asc,
    ));

    assert_eq!(
        ast.validate(),
        Err(BuildError::DistinctOnOrderMismatch { position: 0 })
    );
}

#[test]
fn a_distinct_ordering_matches_a_projected_aggregate() {
    // The ordering criterion repeats the aggregate the projection list
    // exposes, so the DISTINCT rule comparing assembled expressions accepts
    // it: the statement deduplicates rows it sorts by the counted value.
    let mut ast = empty_ast();
    ast.from = Some(TableRef::plain("orders"));
    ast.distinct = true;
    ast.projections.push(count_star());
    ast.order_by
        .push(OrderCriterion::new(count_star(), OrderDirection::Desc));

    assert_eq!(ast.validate(), Ok(()));
}

#[test]
fn a_distinct_ordering_matches_a_window_over_the_other_node_shape() {
    // The projection partitions the window by the schema-marker shape of a
    // column while the ordering criterion partitions the same window by the
    // table-qualified shape of that column, which is what the public
    // correlation escape rewrites every bare reference into. Both spellings
    // name one value of one relation, so the canonical comparison accepts the
    // statement: a window node hides the column references the normalisation
    // exists to resolve.
    let mut ast = empty_ast();
    ast.from = Some(TableRef::plain("orders"));
    ast.distinct = true;
    ast.projections.push(window_partitioned_by(ExprKind::Column {
        table: "orders",
        name: "user_id",
    }));
    ast.order_by.push(OrderCriterion::new(
        window_partitioned_by(ExprKind::QualifiedColumn {
            table: "orders",
            column: "user_id",
        }),
        OrderDirection::Asc,
    ));

    assert_eq!(ast.validate(), Ok(()));
}

#[test]
fn a_distinct_ordering_of_a_window_partitioned_by_another_column_is_rejected() {
    // The canonical comparison resolves the two spellings of one column; it does
    // not make distinct columns match, so the rule still rejects an ordering
    // criterion whose window partitions the rows by another column than the
    // projected window does.
    let mut ast = empty_ast();
    ast.from = Some(TableRef::plain("orders"));
    ast.distinct = true;
    ast.projections.push(window_partitioned_by(ExprKind::Column {
        table: "orders",
        name: "user_id",
    }));
    ast.order_by.push(OrderCriterion::new(
        window_partitioned_by(ExprKind::QualifiedColumn {
            table: "orders",
            column: "total",
        }),
        OrderDirection::Asc,
    ));

    assert_eq!(
        ast.validate(),
        Err(BuildError::DistinctOrderNotProjected { position: 0 })
    );
}

#[test]
fn a_distinct_ordering_matches_a_subquery_written_through_the_other_node_shape() {
    // The projection writes the operand of the membership predicate as a bare
    // column reference while the ordering criterion writes the same predicate
    // through the public correlation escape, whose rewrite turns that reference
    // into a table-qualified one. Both clauses compute the same value, so the
    // DISTINCT rule accepts the ordering.
    let inner = relation_statement("orders", "id");
    let projection = membership_predicate(bare_column("orders", "user_id"), inner.clone());
    let criterion = membership_predicate(qualified_column("orders", "user_id"), inner);

    let mut ast = empty_ast();
    ast.from = Some(TableRef::plain("orders"));
    ast.distinct = true;
    ast.projections.push(projection);
    ast.order_by
        .push(OrderCriterion::new(criterion, OrderDirection::Asc));

    assert_eq!(ast.validate(), Ok(()));
}

#[test]
fn a_distinct_ordering_matches_a_lag_window_over_the_other_node_shape() {
    // The correlation escape rewrites the column a `LAG` reads, the column the
    // window partitions by, and the column it orders by into table-qualified
    // references. Both clauses compute the same value, so the DISTINCT rule
    // accepts the ordering.
    let projection = lag_window(
        bare_column("orders", "total"),
        1,
        bare_column("orders", "user_id"),
        bare_column("orders", "id"),
        OrderDirection::Asc,
    );
    let criterion = lag_window(
        qualified_column("orders", "total"),
        1,
        qualified_column("orders", "user_id"),
        qualified_column("orders", "id"),
        OrderDirection::Asc,
    );

    let mut ast = empty_ast();
    ast.from = Some(TableRef::plain("orders"));
    ast.distinct = true;
    ast.projections.push(projection);
    ast.order_by
        .push(OrderCriterion::new(criterion, OrderDirection::Asc));

    assert_eq!(ast.validate(), Ok(()));
}

#[test]
fn a_distinct_ordering_of_a_window_ordered_the_other_way_is_rejected() {
    // The two windows read the same rows, but their orderings run in opposite
    // directions, so `LAG` reads a different row: the two expressions compute
    // different values and the DISTINCT rule keeps rejecting the ordering.
    let projection = lag_window(
        bare_column("orders", "total"),
        1,
        bare_column("orders", "user_id"),
        bare_column("orders", "id"),
        OrderDirection::Asc,
    );
    let criterion = lag_window(
        bare_column("orders", "total"),
        1,
        bare_column("orders", "user_id"),
        bare_column("orders", "id"),
        OrderDirection::Desc,
    );

    let mut ast = empty_ast();
    ast.from = Some(TableRef::plain("orders"));
    ast.distinct = true;
    ast.projections.push(projection);
    ast.order_by
        .push(OrderCriterion::new(criterion, OrderDirection::Asc));

    assert_eq!(
        ast.validate(),
        Err(BuildError::DistinctOrderNotProjected { position: 0 })
    );
}

#[test]
fn a_distinct_ordering_of_a_window_reading_another_row_is_rejected() {
    // The offset selects the row `LAG` reads, so reading the second row back
    // computes a different value than reading the first one.
    let projection = lag_window(
        bare_column("orders", "total"),
        1,
        bare_column("orders", "user_id"),
        bare_column("orders", "id"),
        OrderDirection::Asc,
    );
    let criterion = lag_window(
        bare_column("orders", "total"),
        2,
        bare_column("orders", "user_id"),
        bare_column("orders", "id"),
        OrderDirection::Asc,
    );

    let mut ast = empty_ast();
    ast.from = Some(TableRef::plain("orders"));
    ast.distinct = true;
    ast.projections.push(projection);
    ast.order_by
        .push(OrderCriterion::new(criterion, OrderDirection::Asc));

    assert_eq!(
        ast.validate(),
        Err(BuildError::DistinctOrderNotProjected { position: 0 })
    );
}

/// Builds a bare column reference to `name` of `table`.
fn bare_column(table: &'static str, name: &'static str) -> ExprKind {
    ExprKind::Column { table, name }
}

/// Builds a reference to `column` of `table`, qualified by its table.
fn qualified_column(table: &'static str, column: &'static str) -> ExprKind {
    ExprKind::QualifiedColumn { table, column }
}

/// Builds a membership predicate testing `operand` against the rows of `inner`.
///
/// The nested statement projects one column of `orders`, so the predicate reads the
/// operand against that column; projecting the predicate makes the enclosing
/// statement project a boolean a `DISTINCT` statement may order by.
fn membership_predicate(operand: ExprKind, inner: SelectAst) -> ExprKind {
    ExprKind::Subquery {
        op: SubqueryOp::In,
        operand: Some(Box::new(operand)),
        query: Subquery::new(inner),
        negated: false,
    }
}

/// Builds a `LAG` window reading `column` `offset` rows before the current row.
///
/// The window partitions by `partition` and orders by `ordering` in `direction`, so
/// a test varies the column references independently while every other clause of
/// the window stays as written.
fn lag_window(
    column: ExprKind,
    offset: u32,
    partition: ExprKind,
    ordering: ExprKind,
    direction: OrderDirection,
) -> ExprKind {
    ExprKind::Window {
        kind: Box::new(crate::expr::WindowExprKind::Lag {
            column,
            offset: Some(offset),
        }),
        partition_by: vec![partition],
        order_by: vec![OrderCriterion::new(ordering, direction)],
        frame: None,
    }
}

/// Builds a `ROW_NUMBER()` window partitioned by `partition`.
///
/// The window carries no ordering and no frame, so two windows built by the
/// helper differ exactly in the partition expression the test supplies.
fn window_partitioned_by(partition: ExprKind) -> ExprKind {
    ExprKind::Window {
        kind: Box::new(crate::expr::WindowExprKind::RowNumber),
        partition_by: vec![partition],
        order_by: Vec::new(),
        frame: None,
    }
}