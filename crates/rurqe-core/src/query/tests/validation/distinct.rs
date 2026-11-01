//! `DISTINCT` and `DISTINCT ON` ordering rules.
//!
//! `DISTINCT` requires every ordering expression to appear in the projection
//! list, and `DISTINCT ON` requires the leading ordering criteria to repeat the
//! deduplicating expressions. Both rules are checked before a statement is
//! rendered, so the tests below assemble the clauses on the statement data as
//! well as through the fluent builders.

use super::super::{MockDialect, Order, OrderAlias, OrderId, OrderTotal, OrderUserId};
use super::{count_star, empty_ast};
use crate::expr::{ExprKind, OrderCriterion, OrderDirection};
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
