//! Grouping coverage of a grouped statement.
//!
//! `.group_by(...)` drops the per-row value of every column the clause does not
//! name, so a projection, a `HAVING` predicate, an ordering criterion, or a
//! deduplication reading such a column names a value the grouping no longer
//! reports. The tests below pin the accepted grouped shapes beside the exact
//! rejections.

use super::super::{MockDialect, Order, OrderTotal, OrderUserId};
use super::{count_star, empty_ast, orders_statement, relation_statement, scalar_subquery, sum_of};
use crate::expr::{
    BinaryOp, Expr, ExprKind, JoinAst, JoinKind, OrderCriterion, OrderDirection, SqlVal,
};
use crate::query::*;
use crate::schema::TableRef;

#[test]
fn a_grouped_aggregate_projection_is_accepted() {
    let built = SelectQuery::<MockDialect>::new()
        .from::<Order>()
        .group_by(OrderUserId)
        .select((OrderUserId, Expr::count_all(), Expr::sum::<OrderTotal>()))
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "SELECT \"user_id\", COUNT(*), SUM(\"total\") FROM \"orders\" GROUP BY \"user_id\""
    );
    assert!(built.params.is_empty());
}

#[test]
fn a_having_predicate_reading_a_grouped_column_is_accepted() {
    let built = SelectQuery::<MockDialect>::new()
        .from::<Order>()
        .group_by(OrderUserId)
        .select((OrderUserId, Expr::count_all()))
        .having(Expr::count_all().gt(1i64))
        .having_col(OrderUserId, Expr::column::<OrderUserId>().gt(10i64))
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "SELECT \"user_id\", COUNT(*) FROM \"orders\" GROUP BY \"user_id\" \
         HAVING ((COUNT(*) > $1) AND (\"user_id\" > $2))"
    );
    assert_eq!(built.params, vec![SqlVal::I64(1), SqlVal::I64(10)]);
}

#[test]
fn a_having_predicate_reading_an_ungrouped_column_is_rejected_before_rendering() {
    // The grouping dropped the per-row value `total` would name, so the predicate
    // cannot read it: the columns of a group are the aggregated and the grouped ones.
    let query = SelectQuery::<MockDialect>::new()
        .from::<Order>()
        .group_by(OrderUserId)
        .select((OrderUserId,))
        .having_col(
            OrderUserId,
            Expr::column::<OrderUserId>()
                .gt(10i64)
                .and(Expr::column::<OrderTotal>().gt(100.0).without_scope()),
        );

    assert_eq!(
        query.build(),
        Err(BuildError::UngroupedHavingColumn {
            table: "orders",
            column: "total",
        })
    );
}

#[test]
fn a_grouped_statement_renders_having_ordering_limit_and_offset() {
    // The grouped query keeps its per-group predicates, deterministic ordering, and
    // pagination after `.group_by(...)` transitioned the typestate, so the tail
    // clauses render in their fixed SQL positions.
    let built = SelectQuery::<MockDialect>::new()
        .from::<Order>()
        .group_by(OrderUserId)
        .select((OrderUserId, Expr::count_all()))
        .having(Expr::count_all().gt(1i64))
        .order_by_desc(OrderUserId)
        .limit(5)
        .offset(10)
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "SELECT \"user_id\", COUNT(*) FROM \"orders\" GROUP BY \"user_id\" \
         HAVING (COUNT(*) > $1) ORDER BY \"user_id\" DESC LIMIT 5 OFFSET 10"
    );
    assert_eq!(built.params, vec![SqlVal::I64(1)]);
}

#[test]
fn a_having_predicate_reading_a_grouped_column_is_accepted_at_the_ast_level() {
    let mut ast = orders_statement(vec![ExprKind::Column {
        table: "orders",
        name: "user_id",
    }]);
    ast.group_by = vec![ExprKind::Column {
        table: "orders",
        name: "user_id",
    }];
    ast.having = Some(ExprKind::Binary {
        op: BinaryOp::Gt,
        lhs: Box::new(ExprKind::Column {
            table: "orders",
            name: "user_id",
        }),
        rhs: Box::new(ExprKind::Value(SqlVal::I64(10))),
    });

    assert_eq!(ast.validate(), Ok(()));
}

#[test]
fn a_having_predicate_matches_a_grouped_column_through_the_other_node_shape() {
    // The grouping records the schema-marker shape while the predicate reads the
    // alias-qualified shape of the same column of the same relation.
    let mut ast = orders_statement(vec![ExprKind::Column {
        table: "orders",
        name: "user_id",
    }]);
    ast.group_by = vec![ExprKind::Column {
        table: "orders",
        name: "user_id",
    }];
    ast.having = Some(ExprKind::Binary {
        op: BinaryOp::Eq,
        lhs: Box::new(ExprKind::QualifiedColumn {
            table: "orders",
            column: "user_id",
        }),
        rhs: Box::new(ExprKind::Value(SqlVal::I64(7))),
    });

    assert_eq!(ast.validate(), Ok(()));
}

#[test]
fn a_having_predicate_over_an_aggregate_is_accepted_at_the_ast_level() {
    let mut ast = orders_statement(vec![ExprKind::Column {
        table: "orders",
        name: "user_id",
    }]);
    ast.group_by = vec![ExprKind::Column {
        table: "orders",
        name: "user_id",
    }];
    ast.having = Some(ExprKind::Binary {
        op: BinaryOp::Gt,
        lhs: Box::new(count_star()),
        rhs: Box::new(ExprKind::Value(SqlVal::I64(1))),
    });

    assert_eq!(ast.validate(), Ok(()));
}

#[test]
fn a_having_predicate_reading_an_ungrouped_ast_column_is_rejected() {
    let mut ast = orders_statement(vec![ExprKind::Column {
        table: "orders",
        name: "user_id",
    }]);
    ast.group_by = vec![ExprKind::Column {
        table: "orders",
        name: "user_id",
    }];
    ast.having = Some(ExprKind::Binary {
        op: BinaryOp::And,
        lhs: Box::new(ExprKind::Binary {
            op: BinaryOp::Gt,
            lhs: Box::new(ExprKind::Column {
                table: "orders",
                name: "user_id",
            }),
            rhs: Box::new(ExprKind::Value(SqlVal::I64(10))),
        }),
        rhs: Box::new(ExprKind::Binary {
            op: BinaryOp::Gt,
            lhs: Box::new(ExprKind::Column {
                table: "orders",
                name: "total",
            }),
            rhs: Box::new(ExprKind::Value(SqlVal::F64(100.0))),
        }),
    });

    assert_eq!(
        ast.validate(),
        Err(BuildError::UngroupedHavingColumn {
            table: "orders",
            column: "total",
        })
    );
}

#[test]
fn a_grouped_statement_projecting_an_ungrouped_column_is_rejected() {
    // The grouping dropped the per-row value `total` would name, so the statement
    // has no per-group value to report for the column projected beside it.
    let mut ast = orders_statement(vec![
        ExprKind::Column {
            table: "orders",
            name: "user_id",
        },
        ExprKind::Column {
            table: "orders",
            name: "total",
        },
    ]);
    ast.group_by = vec![ExprKind::Column {
        table: "orders",
        name: "user_id",
    }];

    assert_eq!(
        ast.validate(),
        Err(BuildError::UngroupedColumn {
            table: "orders",
            column: "total",
        })
    );
}

#[test]
fn a_grouped_statement_ordering_by_an_ungrouped_column_is_rejected() {
    let mut ast = orders_statement(vec![
        ExprKind::Column {
            table: "orders",
            name: "user_id",
        },
        count_star(),
    ]);
    ast.group_by = vec![ExprKind::Column {
        table: "orders",
        name: "user_id",
    }];
    ast.order_by.push(OrderCriterion::new(
        ExprKind::Column {
            table: "orders",
            name: "total",
        },
        OrderDirection::Asc,
    ));

    assert_eq!(
        ast.validate(),
        Err(BuildError::UngroupedColumn {
            table: "orders",
            column: "total",
        })
    );
}

#[test]
fn a_grouped_statement_deduplicating_by_an_ungrouped_column_is_rejected() {
    let mut ast = orders_statement(vec![
        ExprKind::Column {
            table: "orders",
            name: "user_id",
        },
        count_star(),
    ]);
    ast.group_by = vec![ExprKind::Column {
        table: "orders",
        name: "user_id",
    }];
    ast.distinct_on.push(ExprKind::Column {
        table: "orders",
        name: "total",
    });

    assert_eq!(
        ast.validate(),
        Err(BuildError::UngroupedColumn {
            table: "orders",
            column: "total",
        })
    );
}

#[test]
fn a_grouped_statement_ordering_by_an_aggregate_is_accepted() {
    // The aggregate is computed over the groups the clause produced, so ordering by
    // it stays valid beside the grouped column.
    let mut ast = orders_statement(vec![
        ExprKind::Column {
            table: "orders",
            name: "user_id",
        },
        sum_of("orders", "total"),
    ]);
    ast.group_by = vec![ExprKind::Column {
        table: "orders",
        name: "user_id",
    }];
    ast.order_by.push(OrderCriterion::new(
        sum_of("orders", "total"),
        OrderDirection::Desc,
    ));

    assert_eq!(ast.validate(), Ok(()));
}

#[test]
fn a_grouped_projection_matches_a_grouping_written_through_the_other_node_shape() {
    // The grouping records the alias-qualified shape of the column while the
    // projection reads the schema-marker shape, so the canonical comparison
    // resolves both to one grouped column.
    let mut ast = orders_statement(vec![
        ExprKind::Column {
            table: "orders",
            name: "user_id",
        },
        sum_of("orders", "total"),
    ]);
    ast.group_by = vec![ExprKind::QualifiedColumn {
        table: "orders",
        column: "user_id",
    }];

    assert_eq!(ast.validate(), Ok(()));
}

#[test]
fn build_error_display_ungrouped_column() {
    let err = BuildError::UngroupedColumn {
        table: "orders",
        column: "status",
    };
    let msg = err.to_string();
    assert!(msg.contains("`orders.status`"));
    assert!(msg.contains("neither aggregated nor grouped"));
    assert!(msg.contains("`.group_by(...)`"));
}

#[test]
fn a_grouped_statement_accepts_a_scalar_subquery_correlated_to_the_grouped_column() {
    // The scalar subquery projects a value of the statement it declares, while its
    // filter reaches the grouped column of the enclosing statement by correlation,
    // so neither rule rejects the projection.
    let mut inner = relation_statement("orders", "total");
    inner.filter = Some(ExprKind::Binary {
        op: BinaryOp::Eq,
        lhs: Box::new(ExprKind::Column {
            table: "orders",
            name: "user_id",
        }),
        rhs: Box::new(ExprKind::Column {
            table: "users",
            name: "id",
        }),
    });

    let mut ast = empty_ast();
    ast.from = Some(TableRef::plain("users"));
    ast.projections = vec![
        ExprKind::Column {
            table: "users",
            name: "id",
        },
        Expr::<f64>::from_kind(scalar_subquery(inner)).into_kind(),
    ];
    ast.group_by = vec![ExprKind::Column {
        table: "users",
        name: "id",
    }];

    assert_eq!(ast.validate(), Ok(()));
}

#[test]
fn a_lateral_subquery_correlated_to_the_grouped_column_is_accepted() {
    // A lateral subquery reads the relations in scope where it is written, so
    // correlating to the grouped column of the statement joining it stays valid
    // while its aggregate is governed by its own scope.
    let mut inner = relation_statement("orders", "total");
    inner.filter = Some(ExprKind::Binary {
        op: BinaryOp::Eq,
        lhs: Box::new(ExprKind::Column {
            table: "orders",
            name: "user_id",
        }),
        rhs: Box::new(ExprKind::Column {
            table: "users",
            name: "id",
        }),
    });

    let mut ast = empty_ast();
    ast.from = Some(TableRef::plain("users"));
    ast.joins.push(JoinAst {
        kind: JoinKind::LeftLateral,
        table: TableRef::plain("recent"),
        lateral: Some(inner),
        on: None,
    });
    ast.projections = vec![
        ExprKind::Column {
            table: "users",
            name: "id",
        },
        sum_of("recent", "total"),
    ];
    ast.group_by = vec![ExprKind::Column {
        table: "users",
        name: "id",
    }];

    assert_eq!(ast.validate(), Ok(()));
}

#[test]
fn a_grouped_statement_deduplicating_by_a_grouped_column_is_accepted() {
    // The deduplication and the ordering name the grouped column, so both the
    // prefix alignment and the grouped-column coverage hold together.
    let mut ast = orders_statement(vec![
        ExprKind::Column {
            table: "orders",
            name: "user_id",
        },
        count_star(),
    ]);
    ast.group_by = vec![ExprKind::Column {
        table: "orders",
        name: "user_id",
    }];
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

    assert_eq!(ast.validate(), Ok(()));
}

#[test]
fn a_grouped_statement_rejects_a_bare_column_hidden_in_a_computed_projection() {
    // The grouping dropped the per-row value `total` would name, and wrapping the
    // reference in a function does not aggregate it, so the projection is rejected
    // through the column the expression reads.
    let mut ast = orders_statement(vec![
        ExprKind::Column {
            table: "orders",
            name: "user_id",
        },
        Expr::<f64>::from_kind(ExprKind::Function {
            name: "COALESCE",
            args: vec![
                ExprKind::Column {
                    table: "orders",
                    name: "total",
                },
                ExprKind::Value(SqlVal::F64(0.0)),
            ],
        })
        .into_kind(),
    ]);
    ast.group_by = vec![ExprKind::Column {
        table: "orders",
        name: "user_id",
    }];

    assert_eq!(
        ast.validate(),
        Err(BuildError::UngroupedColumn {
            table: "orders",
            column: "total",
        })
    );
}

#[test]
fn a_grouped_statement_nested_in_a_projection_is_validated_itself() {
    // The embedded statement is validated in full, so the grouped-column coverage
    // reaches one level deeper than the projection list embedding it.
    let mut inner = relation_statement("orders", "user_id");
    inner.projections = vec![
        ExprKind::Column {
            table: "orders",
            name: "user_id",
        },
        ExprKind::Column {
            table: "orders",
            name: "total",
        },
        count_star(),
    ];
    inner.group_by = vec![ExprKind::Column {
        table: "orders",
        name: "user_id",
    }];

    let mut ast = relation_statement("users", "id");
    ast.projections.push(scalar_subquery(inner));

    assert_eq!(
        ast.validate(),
        Err(BuildError::UngroupedColumn {
            table: "orders",
            column: "total",
        })
    );
}

/// `ABS(orders.<column>)`, a computed expression over one column of `orders`.
fn abs_of(column: &'static str) -> ExprKind {
    ExprKind::Function {
        name: "ABS",
        args: vec![ExprKind::Column {
            table: "orders",
            name: column,
        }],
    }
}

#[test]
fn a_grouped_computed_expression_covers_the_equal_expression_everywhere() {
    let mut ast = orders_statement(vec![abs_of("total"), count_star()]);
    ast.group_by = vec![abs_of("total")];
    ast.having = Some(ExprKind::Binary {
        op: BinaryOp::Gt,
        lhs: Box::new(abs_of("total")),
        rhs: Box::new(ExprKind::Value(SqlVal::F64(1.0))),
    });
    ast.order_by
        .push(OrderCriterion::new(abs_of("total"), OrderDirection::Asc));

    assert_eq!(ast.validate(), Ok(()));
}

#[test]
fn a_variation_of_a_grouped_computed_expression_is_rejected() {
    // The projection computes over `user_id`, a column neither grouped nor covered
    // by the grouped expression over `total`.
    let mut ast = orders_statement(vec![abs_of("user_id"), count_star()]);
    ast.group_by = vec![abs_of("total")];

    let error = ast
        .validate()
        .expect_err("a variation of the grouped expression");
    assert!(matches!(error, BuildError::UngroupedExpression { .. }));
    assert!(error.to_string().contains("ABS"));
}

#[test]
fn a_bare_column_beside_a_grouped_computed_expression_is_rejected() {
    let mut ast = orders_statement(vec![
        ExprKind::Column {
            table: "orders",
            name: "total",
        },
        count_star(),
    ]);
    ast.group_by = vec![abs_of("total")];

    assert_eq!(
        ast.validate(),
        Err(BuildError::UngroupedColumn {
            table: "orders",
            column: "total",
        })
    );
}

#[test]
fn a_having_variation_of_a_grouped_computed_expression_is_rejected() {
    let mut ast = orders_statement(vec![abs_of("total"), count_star()]);
    ast.group_by = vec![abs_of("total")];
    ast.having = Some(ExprKind::Binary {
        op: BinaryOp::Gt,
        lhs: Box::new(abs_of("user_id")),
        rhs: Box::new(ExprKind::Value(SqlVal::F64(1.0))),
    });

    assert_eq!(
        ast.validate(),
        Err(BuildError::UngroupedHavingColumn {
            table: "orders",
            column: "user_id",
        })
    );
}

#[test]
fn the_builder_refuses_an_ungrouped_variation_of_a_grouped_expression() {
    let query = SelectQuery::<MockDialect>::new()
        .from::<Order>()
        .group_by(
            Expr::<f64>::fragments("ABS(")
                .operand(OrderTotal)
                .text(")")
                .end(),
        )
        .select((
            Expr::<f64>::fragments("ABS(")
                .operand(OrderUserId)
                .text(")")
                .end(),
            Expr::count_all(),
        ));

    assert!(matches!(
        query.build(),
        Err(BuildError::UngroupedExpression { .. })
    ));
}
