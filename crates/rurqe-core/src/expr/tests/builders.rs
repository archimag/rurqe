use super::*;

crate::table!(
    Order => "orders" {
        OrderTotal => "total": f64,
        OrderTax => "tax": f64,
        OrderQty => "qty": i64,
    }
);

#[test]
fn column_expression_carries_static_name() {
    assert_eq!(
        Expr::column::<UserId>().kind(),
        &ExprKind::Column {
            table: "users",
            name: "id",
        }
    );
    assert_eq!(
        Expr::column::<UserName>().kind(),
        &ExprKind::Column {
            table: "users",
            name: "name",
        }
    );
}

#[test]
fn qualified_column_expression_carries_table_and_column_names() {
    assert_eq!(
        Expr::qualified_column::<User, UserId>().kind(),
        &ExprKind::QualifiedColumn {
            table: "users",
            column: "id",
        }
    );
}

#[test]
fn qualified_column_expression_is_typed_by_the_column() {
    let predicate = Expr::qualified_column::<User, UserName>().eq("root");

    assert_eq!(
        predicate.kind(),
        &ExprKind::Binary {
            op: BinaryOp::Eq,
            lhs: Box::new(ExprKind::QualifiedColumn {
                table: "users",
                column: "name",
            }),
            rhs: Box::new(ExprKind::Value(SqlVal::Text("root".to_owned()))),
        }
    );
}

#[test]
fn comparison_builds_binary_expression() {
    let predicate = Expr::column::<UserId>().eq(42i64);

    assert_eq!(
        predicate.kind(),
        &ExprKind::Binary {
            op: BinaryOp::Eq,
            lhs: Box::new(ExprKind::Column {
                table: "users",
                name: "id",
            }),
            rhs: Box::new(ExprKind::Value(SqlVal::I64(42))),
        }
    );
}

#[test]
fn string_column_accepts_str_literals() {
    let predicate = Expr::column::<UserName>().ne("root");

    assert_eq!(
        predicate.kind(),
        &ExprKind::Binary {
            op: BinaryOp::Ne,
            lhs: Box::new(ExprKind::Column {
                table: "users",
                name: "name",
            }),
            rhs: Box::new(ExprKind::Value(SqlVal::Text("root".to_owned()))),
        }
    );
}

#[test]
fn predicates_compose_with_and_and_or() {
    let predicate = Expr::column::<UserActive>()
        .eq(true)
        .and(Expr::column::<UserId>().gt(10i64))
        .or(Expr::column::<UserName>().lt("zzz"));

    let ExprKind::Binary { op, lhs, rhs } = predicate.kind() else {
        panic!("expected a binary expression");
    };
    assert_eq!(*op, BinaryOp::Or);
    assert_eq!(
        &**rhs,
        &ExprKind::Binary {
            op: BinaryOp::Lt,
            lhs: Box::new(ExprKind::Column {
                table: "users",
                name: "name",
            }),
            rhs: Box::new(ExprKind::Value(SqlVal::Text("zzz".to_owned()))),
        }
    );

    let ExprKind::Binary { op, .. } = &**lhs else {
        panic!("expected a nested binary expression");
    };
    assert_eq!(*op, BinaryOp::And);
}

#[test]
fn aggregate_expressions_build_expected_ast() {
    assert_eq!(
        Expr::count_all().kind(),
        &ExprKind::Aggregate {
            func: AggregateFunc::Count,
            distinct: false,
            arg: None,
            filter: None,
        }
    );
    assert_eq!(
        Expr::count::<UserId>().kind(),
        &ExprKind::Aggregate {
            func: AggregateFunc::Count,
            distinct: false,
            arg: Some(Box::new(ExprKind::Column {
                table: "users",
                name: "id",
            })),
            filter: None,
        }
    );
    assert_eq!(
        Expr::sum::<UserId>().kind(),
        &ExprKind::Aggregate {
            func: AggregateFunc::Sum,
            distinct: false,
            arg: Some(Box::new(ExprKind::Column {
                table: "users",
                name: "id",
            })),
            filter: None,
        }
    );
    assert_eq!(
        Expr::avg::<UserId>().kind(),
        &ExprKind::Aggregate {
            func: AggregateFunc::Avg,
            distinct: false,
            arg: Some(Box::new(ExprKind::Column {
                table: "users",
                name: "id",
            })),
            filter: None,
        }
    );
    assert_eq!(
        Expr::min::<UserId>().kind(),
        &ExprKind::Aggregate {
            func: AggregateFunc::Min,
            distinct: false,
            arg: Some(Box::new(ExprKind::Column {
                table: "users",
                name: "id",
            })),
            filter: None,
        }
    );
    assert_eq!(
        Expr::max::<UserName>().kind(),
        &ExprKind::Aggregate {
            func: AggregateFunc::Max,
            distinct: false,
            arg: Some(Box::new(ExprKind::Column {
                table: "users",
                name: "name",
            })),
            filter: None,
        }
    );
}

#[test]
fn case_expression_builds_searched_branches() {
    let expr = Expr::<String>::case()
        .when(Expr::column::<UserId>().gt(100i64), "vip")
        .when(Expr::column::<UserId>().gt(10i64), "regular")
        .else_("newcomer")
        .end();

    let ExprKind::Case {
        branches,
        else_branch,
    } = expr.kind()
    else {
        panic!("expected a CASE expression");
    };

    assert_eq!(branches.len(), 2);
    assert_eq!(
        branches[0].result,
        ExprKind::Value(SqlVal::Text("vip".to_owned()))
    );
    assert_eq!(
        branches[1].condition,
        ExprKind::Binary {
            op: BinaryOp::Gt,
            lhs: Box::new(ExprKind::Column {
                table: "users",
                name: "id",
            }),
            rhs: Box::new(ExprKind::Value(SqlVal::I64(10))),
        }
    );
    assert_eq!(
        else_branch.as_deref(),
        Some(&ExprKind::Value(SqlVal::Text("newcomer".to_owned())))
    );
}

#[test]
fn empty_membership_lists_degrade_into_a_constant_predicate() {
    let nothing = Expr::column::<UserId>().in_list(Vec::<i64>::new());
    assert_eq!(
        nothing.kind(),
        &ExprKind::Raw {
            fragments: vec!["1 = 0".to_owned()],
            params: Vec::new(),
        }
    );

    let everything = Expr::column::<UserId>().not_in_list(Vec::<i64>::new());
    assert_eq!(
        everything.kind(),
        &ExprKind::Raw {
            fragments: vec!["1 = 1".to_owned()],
            params: Vec::new(),
        }
    );
}

#[test]
fn a_membership_list_carries_the_tables_its_candidates_read() {
    // A list of bound values references no table, so the predicate keeps the
    // scope of the tested column.
    let values: Expr<bool, User> = UserId.in_list(vec![1i64, 2i64]);
    assert!(matches!(
        values.kind(),
        ExprKind::InList { negated: false, .. }
    ));

    // A candidate reading a column of another table extends the scope with that
    // table, so the enclosing query has to select from it as well.
    let candidates: Expr<bool, (User, Order)> = UserId.in_list(vec![Expr::column::<OrderQty>()]);
    assert!(matches!(
        candidates.kind(),
        ExprKind::InList { negated: false, .. }
    ));

    let negated: Expr<bool, (User, Order)> = UserId.not_in_list(vec![Expr::column::<OrderQty>()]);
    assert!(matches!(
        negated.kind(),
        ExprKind::InList { negated: true, .. }
    ));
}

#[test]
fn compatible_operands_are_accepted() {
    fn assert_compatible<ColType, ValType>()
    where
        ValType: SqlCompatible<ColType, ValType>,
    {
    }

    assert_compatible::<i64, i64>();
    assert_compatible::<bool, bool>();
    assert_compatible::<String, String>();
    assert_compatible::<String, &'static str>();
    assert_compatible::<i64, Expr<i64>>();
}

#[test]
fn expressions_can_be_compared_with_other_expressions() {
    let predicate = Expr::column::<UserId>().eq(Expr::column::<UserId>());

    assert_eq!(
        predicate.kind(),
        &ExprKind::Binary {
            op: BinaryOp::Eq,
            lhs: Box::new(ExprKind::Column {
                table: "users",
                name: "id",
            }),
            rhs: Box::new(ExprKind::Column {
                table: "users",
                name: "id",
            }),
        }
    );
}

#[test]
fn like_and_ilike_build_pattern_predicates() {
    let predicate = Expr::column::<UserName>().like("A%");
    assert_eq!(
        predicate.kind(),
        &ExprKind::Like {
            operand: Box::new(ExprKind::Column {
                table: "users",
                name: "name",
            }),
            pattern: Box::new(ExprKind::Value(SqlVal::Text("A%".to_owned()))),
            case_insensitive: false,
        }
    );

    let predicate = Expr::column::<UserName>().ilike("a%");
    assert!(matches!(
        predicate.kind(),
        ExprKind::Like {
            case_insensitive: true,
            ..
        }
    ));
}

#[test]
fn raw_templates_report_placeholder_mismatches() {
    let expression: Expr<i64> =
        Expr::raw("COUNT(*)", Vec::new()).expect("a template without placeholders");

    assert_eq!(
        expression.kind(),
        &ExprKind::Raw {
            fragments: vec!["COUNT(*)".to_owned()],
            params: Vec::new(),
        }
    );

    assert_eq!(
        Expr::<i64>::raw("rate > ?", Vec::new()).err(),
        Some(RawSqlError::PlaceholderCount {
            expected: 1,
            provided: 0,
        })
    );
}

#[test]
fn column_markers_are_expressions() {
    assert_eq!(UserId.eq(7i64), Expr::column::<UserId>().eq(7i64));
    assert_eq!(UserId.ne(7i64), Expr::column::<UserId>().ne(7i64));
    assert_eq!(UserId.gt(7i64), Expr::column::<UserId>().gt(7i64));
    assert_eq!(UserId.lt(7i64), Expr::column::<UserId>().lt(7i64));
    assert_eq!(UserName.is_null(), Expr::column::<UserName>().is_null());
    assert_eq!(
        UserName.is_not_null(),
        Expr::column::<UserName>().is_not_null()
    );
    assert_eq!(
        UserId.between(1i64, 9i64),
        Expr::column::<UserId>().between(1i64, 9i64)
    );
    assert_eq!(
        UserId.in_list(vec![1i64, 2i64]),
        Expr::column::<UserId>().in_list(vec![1i64, 2i64])
    );
    assert_eq!(
        UserId.not_in_list(vec![1i64]),
        Expr::column::<UserId>().not_in_list(vec![1i64])
    );
}

#[test]
fn string_columns_match_like_patterns() {
    assert_eq!(UserName.like("A%"), Expr::column::<UserName>().like("A%"));
    assert_eq!(UserName.ilike("a%"), Expr::column::<UserName>().ilike("a%"));
}

#[test]
fn aggregate_helpers_build_expected_expressions() {
    assert_eq!(count(), Expr::<i64>::count_all());
    assert_eq!(count_all(), Expr::<i64>::count_all());
    assert_eq!(
        count_distinct(UserId),
        Expr::<i64>::count_distinct::<UserId>()
    );
    assert_eq!(sum(UserId), Expr::<i64>::sum::<UserId>());
    assert_eq!(sum_distinct(UserId), Expr::<i64>::sum_distinct::<UserId>());
    assert_eq!(avg(UserId), Expr::<f64>::avg::<UserId>());
    assert_eq!(avg_distinct(UserId), Expr::<f64>::avg_distinct::<UserId>());
    assert_eq!(min(UserId), Expr::<i64>::min::<UserId>());
    assert_eq!(max(UserName), Expr::<String>::max::<UserName>());
}

#[test]
fn column_aggregates_carry_the_table_of_their_column() {
    // An aggregate reading a column references the table of that column, so a
    // statement whose scope lacks the table rejects it instead of projecting a
    // value the database cannot compute.
    let _: Expr<i64, User, DirectAggregate> = count_distinct(UserId);
    let _: Expr<i64, User, DirectAggregate> = sum(UserId);
    let _: Expr<i64, User, DirectAggregate> = sum_distinct(UserId);
    let _: Expr<f64, User, DirectAggregate> = avg(UserId);
    let _: Expr<f64, User, DirectAggregate> = avg_distinct(UserId);
    let _: Expr<i64, User, DirectAggregate> = min(UserId);
    let _: Expr<String, User, DirectAggregate> = max(UserName);

    // The constructors of `Expr` keep the same scope and the same marker as the
    // free functions: a call keeps the aggregate at its root, so the optional
    // clauses of the call apply to it directly.
    let _: Expr<i64, User, DirectAggregate> = Expr::<i64>::count::<UserId>();
    let _: Expr<i64, User, DirectAggregate> = Expr::<i64>::count_distinct::<UserId>();
    let _: Expr<i64, User, DirectAggregate> = Expr::<i64>::sum::<UserId>();
    let _: Expr<i64, User, DirectAggregate> = Expr::<i64>::sum_distinct::<UserId>();
    let _: Expr<f64, User, DirectAggregate> = Expr::<f64>::avg::<UserId>();
    let _: Expr<f64, User, DirectAggregate> = Expr::<f64>::avg_distinct::<UserId>();
    let _: Expr<i64, User, DirectAggregate> = Expr::<i64>::min::<UserId>();
    let _: Expr<String, User, DirectAggregate> = Expr::<String>::max::<UserName>();

    // `COUNT(*)` reads no column, so it references no table and stays constant
    // within every group.
    let _: Expr<i64, (), DirectAggregate> = count_all();
    let _: Expr<i64, (), DirectAggregate> = count();
}

#[test]
fn aggregate_scopes_merge_with_the_columns_their_predicates_read() {
    let filtered: Expr<i64, (User, User), FilteredAggregate> =
        sum(UserId).filter_where(UserId.gt(0i64));

    assert!(matches!(
        filtered.kind(),
        ExprKind::Aggregate {
            filter: Some(_),
            ..
        }
    ));
}

#[test]
fn distinct_and_filtered_aggregates_build_expected_ast() {
    let distinct_count = Expr::count::<UserId>().distinct();
    assert_eq!(
        distinct_count.kind(),
        &ExprKind::Aggregate {
            func: AggregateFunc::Count,
            distinct: true,
            arg: Some(Box::new(ExprKind::Column {
                table: "users",
                name: "id",
            })),
            filter: None,
        }
    );

    let filtered = sum(UserId).filter_where(UserId.gt(10i64));
    assert_eq!(
        filtered.kind(),
        &ExprKind::Aggregate {
            func: AggregateFunc::Sum,
            distinct: false,
            arg: Some(Box::new(ExprKind::Column {
                table: "users",
                name: "id",
            })),
            filter: Some(Box::new(ExprKind::Binary {
                op: BinaryOp::Gt,
                lhs: Box::new(ExprKind::Column {
                    table: "users",
                    name: "id",
                }),
                rhs: Box::new(ExprKind::Value(SqlVal::I64(10))),
            })),
        }
    );

    let chained = Expr::count::<UserId>()
        .distinct()
        .filter_where(UserId.gt(0i64))
        .filter_where(UserId.lt(100i64));

    let ExprKind::Aggregate {
        distinct,
        filter: Some(filter),
        ..
    } = chained.kind()
    else {
        panic!("expected aggregate with filter");
    };
    assert!(*distinct);
    assert!(matches!(
        **filter,
        ExprKind::Binary {
            op: BinaryOp::And,
            ..
        }
    ));
}

#[test]
fn expressions_carry_table_scopes_across_operators() {
    // Single-table comparison carries the table scope
    let eq_expr: Expr<bool, User> = Expr::column::<UserId>().eq(42i64);
    assert_eq!(
        eq_expr.kind(),
        &ExprKind::Binary {
            op: BinaryOp::Eq,
            lhs: Box::new(ExprKind::Column {
                table: "users",
                name: "id",
            }),
            rhs: Box::new(ExprKind::Value(SqlVal::I64(42))),
        }
    );

    let marker_eq: Expr<bool, User> = UserId.eq(42i64);
    assert_eq!(marker_eq, eq_expr);

    crate::table!(
        Order => "orders" {
            OrderId => "id": i64,
            OrderUserId => "user_id": i64,
        }
    );

    // Cross-table comparison merges scopes into a tuple
    let cross_eq: Expr<bool, (User, Order)> =
        Expr::column::<UserId>().eq(Expr::column::<OrderUserId>());
    assert!(matches!(
        cross_eq.kind(),
        ExprKind::Binary {
            op: BinaryOp::Eq,
            ..
        }
    ));

    // and / or preserve and merge table scopes
    let and_expr: Expr<bool, (User, Order)> = UserId.eq(1i64).and(OrderUserId.eq(2i64));
    assert!(matches!(
        and_expr.kind(),
        ExprKind::Binary {
            op: BinaryOp::And,
            ..
        }
    ));

    let or_expr: Expr<bool, (User, Order)> = UserId.eq(1i64).or(OrderUserId.eq(2i64));
    assert!(matches!(
        or_expr.kind(),
        ExprKind::Binary {
            op: BinaryOp::Or,
            ..
        }
    ));

    // between preserves table scope
    let between_expr: Expr<bool, User> = UserId.between(1i64, 10i64);
    assert!(matches!(between_expr.kind(), ExprKind::Between { .. }));

    // like and ilike preserve table scope
    let like_expr: Expr<bool, User> = UserName.like("alice%");
    assert!(matches!(
        like_expr.kind(),
        ExprKind::Like {
            case_insensitive: false,
            ..
        }
    ));

    let ilike_expr: Expr<bool, User> = UserName.ilike("alice%");
    assert!(matches!(
        ilike_expr.kind(),
        ExprKind::Like {
            case_insensitive: true,
            ..
        }
    ));

    // is_null and is_not_null preserve table scope
    let null_expr: Expr<bool, User> = UserName.is_null();
    assert!(matches!(
        null_expr.kind(),
        ExprKind::IsNull { negated: false, .. }
    ));

    let not_null_expr: Expr<bool, User> = UserName.is_not_null();
    assert!(matches!(
        not_null_expr.kind(),
        ExprKind::IsNull { negated: true, .. }
    ));
}

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
        fragments[0].contains("FILTER"),
        "the placeholder names the clause it replaced: {fragments:?}"
    );
    assert!(bound.is_empty());

    // The placeholder reaches the rendered statement, so the discarded predicate
    // surfaces in the SQL a caller inspects instead of vanishing in the build.
    let (sql, rendered_params) = render(marked.kind());
    assert_eq!(sql, fragments[0]);
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
        fragments[0].contains("DISTINCT"),
        "the placeholder names the modifier it dropped: {fragments:?}"
    );
    assert!(bound.is_empty());

    // The placeholder reaches the rendered statement, so the dropped modifier
    // surfaces in the SQL a caller inspects instead of vanishing in the build.
    let (sql, rendered_params) = render(marked.kind());
    assert_eq!(sql, fragments[0]);
    assert!(rendered_params.is_empty());
}

#[test]
fn unbalanced_raw_fragments_are_rejected() {
    // The renderer binds a parameter after every fragment except the last, so
    // a pair violating the interleaving invariant would dispatch SQL with a
    // missing or a misplaced placeholder; the mismatch is reported as a typed
    // error instead of dispatching broken SQL.
    let expression = Expr::<i64>::raw_fragments(
        vec!["qty > ".to_owned()],
        vec![SqlVal::I64(1), SqlVal::I64(10)],
    );

    let mut sql = String::new();
    let mut params = Vec::new();
    let error = render_expr::<MockDialect>(&mut sql, &mut params, expression.kind(), &[])
        .expect_err("unbalanced fragments are rejected");

    // One fragment declares no placeholder at all, so the two supplied
    // parameters have nothing to bind to.
    assert_eq!(
        error,
        RawSqlError::PlaceholderCount {
            expected: 0,
            provided: 2,
        }
    );
}

#[test]
fn balanced_raw_fragments_bind_their_parameters_in_order() {
    // One fragment more than parameters: the fragments alternate with the
    // bound values, which is the shape `parse_raw_sql` splits a template into
    // and the shape the renderer binds correctly.
    let expr = Expr::<i64>::raw_fragments(
        vec!["qty + ".to_owned(), " AND tax > ".to_owned(), String::new()],
        vec![SqlVal::I64(1), SqlVal::I64(10)],
    );

    let (sql, params) = render(expr.kind());

    assert_eq!(sql, "qty + ? AND tax > ?");
    assert_eq!(params, vec![SqlVal::I64(1), SqlVal::I64(10)]);
}

#[test]
fn dialect_operators_inherit_the_scope_of_their_operands() {
    // A bound right operand references no table, so the operator keeps exactly
    // the scope of the column it is applied to.
    let matched: Expr<bool, User> =
        Expr::column::<UserName>().dialect_op::<String, bool, _>("~", "^alice");

    assert_eq!(
        matched.kind(),
        &ExprKind::DialectOp {
            op: "~",
            lhs: Box::new(ExprKind::Column {
                table: "users",
                name: "name",
            }),
            rhs: Box::new(ExprKind::Value(SqlVal::Text("^alice".to_owned()))),
        }
    );

    let (sql, params) = render(matched.kind());
    assert_eq!(sql, "(\"name\" ~ ?)");
    assert_eq!(params, vec![SqlVal::Text("^alice".to_owned())]);

    // A right operand reading a column of another table extends the scope with
    // that table, exactly as comparing the two columns does.
    let compared: Expr<i64, (User, Order)> =
        Expr::column::<UserId>().dialect_op::<i64, i64, _>("+", Expr::column::<OrderQty>());

    assert_eq!(
        compared.kind(),
        &ExprKind::DialectOp {
            op: "+",
            lhs: Box::new(ExprKind::Column {
                table: "users",
                name: "id",
            }),
            rhs: Box::new(ExprKind::Column {
                table: "orders",
                name: "qty",
            }),
        }
    );
}

#[test]
fn dialect_operators_keep_the_dialect_marker_of_their_operands() {
    // The dialect-pinned operands are assembled with the crate-internal
    // constructor: the escape hatch builds scope-free constants only, while the
    // scope of a column reference comes from its column marker. The test
    // exercises how the dialect marker survives the operators below.
    let pinned: Expr<i64, User, NonAggregate, MockDialect> = Expr::new(ExprKind::Column {
        table: "users",
        name: "id",
    });
    let pinned_rhs: Expr<i64, User, NonAggregate, MockDialect> = Expr::new(ExprKind::Column {
        table: "users",
        name: "id",
    });

    // A dialect-neutral right operand adds nothing to the marker of the left
    // one, so the operator still requires the dialect its left operand does.
    let appended: Expr<i64, User, NonAggregate, MockDialect> =
        pinned.dialect_op::<i64, i64, _>("%", 2i64);
    assert!(matches!(
        appended.kind(),
        ExprKind::DialectOp { op: "%", .. }
    ));

    // A dialect-neutral left operand keeps the marker of its right-hand side.
    let prepended: Expr<i64, User, NonAggregate, MockDialect> =
        Expr::<i64>::literal(2i64).dialect_op::<i64, i64, _>("%", pinned_rhs);
    assert!(matches!(
        prepended.kind(),
        ExprKind::DialectOp { op: "%", .. }
    ));
}

#[test]
fn quantified_comparisons_wrap_the_quantifier_around_their_operand() {
    // The receiver is the quantified side, so the searched value lands on the
    // left of the comparison and the quantifier wraps the receiver.
    let matched: Expr<bool, Order> =
        Expr::column::<OrderQty>().dialect_quantified::<i64, _>("ANY", 7i64);

    assert_eq!(
        matched.kind(),
        &ExprKind::Binary {
            op: BinaryOp::Eq,
            lhs: Box::new(ExprKind::Value(SqlVal::I64(7))),
            rhs: Box::new(ExprKind::Function {
                name: "ANY",
                args: vec![ExprKind::Column {
                    table: "orders",
                    name: "qty",
                }],
            }),
        }
    );

    let (sql, params) = render(matched.kind());
    assert_eq!(sql, "(? = ANY(\"qty\"))");
    assert_eq!(params, vec![SqlVal::I64(7)]);
}

#[test]
fn quantified_comparisons_inherit_the_scopes_of_both_operands() {
    // A neutral right operand contributes no table, so the scope is the table of
    // the quantified column.
    let over_a_column: Expr<bool, User> =
        Expr::column::<UserName>().dialect_quantified::<String, _>("ANY", "^alice".to_owned());
    assert!(matches!(
        over_a_column.kind(),
        ExprKind::Binary {
            op: BinaryOp::Eq,
            ..
        }
    ));

    // A left operand reading a column extends the merged scope as well, so the
    // comparison references both tables.
    let over_two_tables: Expr<bool, (Order, User)> = Expr::column::<OrderQty>()
        .dialect_quantified::<String, _>("ANY", Expr::column::<UserName>());
    assert!(matches!(
        over_two_tables.kind(),
        ExprKind::Binary {
            op: BinaryOp::Eq,
            ..
        }
    ));
}

#[test]
fn without_scope_qualifies_every_operand_of_a_window_expression() {
    // A window specification names its columns in three places: the operand of the
    // function, the partitioning, and the ordering. All three stay ordinary
    // operands of the statement projecting the window, so a tree freed from its
    // scope has to spell each of them out.
    let window = ExprKind::Window {
        kind: Box::new(WindowExprKind::Lag {
            column: Expr::column::<UserId>().into_kind(),
            offset: Some(2),
        }),
        partition_by: vec![Expr::column::<UserActive>().into_kind()],
        order_by: vec![OrderCriterion::new(
            Expr::column::<UserName>().into_kind(),
            OrderDirection::Desc,
        )
        .nulls(NullsOrder::Last)],
        frame: None,
    };

    let unscoped: Expr<i64, Unscoped> = Expr::from_kind(window).without_scope();

    let ExprKind::Window {
        kind,
        partition_by,
        order_by,
        frame,
    } = unscoped.kind()
    else {
        panic!("expected a window expression, got {:?}", unscoped.kind());
    };

    assert_eq!(
        kind.as_ref(),
        &WindowExprKind::Lag {
            column: ExprKind::QualifiedColumn {
                table: "users",
                column: "id",
            },
            offset: Some(2),
        }
    );
    assert_eq!(
        partition_by,
        &[ExprKind::QualifiedColumn {
            table: "users",
            column: "active",
        }]
    );
    assert_eq!(
        order_by,
        &[OrderCriterion::new(
            ExprKind::QualifiedColumn {
                table: "users",
                column: "name",
            },
            OrderDirection::Desc,
        )
        .nulls(NullsOrder::Last)]
    );
    // The frame counts rows or values and names no column, so it is kept as is.
    assert!(frame.is_none());
}

#[test]
fn without_scope_qualifies_the_expression_a_window_evaluates() {
    // The expression a window evaluates is an operand of the enclosing statement as
    // well, so an aggregate evaluated over the window has to name the table of the
    // column it reads.
    let window = ExprKind::Window {
        kind: Box::new(WindowExprKind::Expr(
            Expr::<i64>::count_distinct::<UserId>().into_kind(),
        )),
        partition_by: Vec::new(),
        order_by: Vec::new(),
        frame: None,
    };

    let unscoped: Expr<i64, Unscoped> = Expr::from_kind(window).without_scope();

    let ExprKind::Window { kind, .. } = unscoped.kind() else {
        panic!("expected a window expression, got {:?}", unscoped.kind());
    };
    let WindowExprKind::Expr(inner) = kind.as_ref() else {
        panic!("expected the aggregate to stay the operand of the window");
    };

    assert_eq!(
        inner,
        &ExprKind::Aggregate {
            func: AggregateFunc::Count,
            distinct: true,
            arg: Some(Box::new(ExprKind::QualifiedColumn {
                table: "users",
                column: "id",
            })),
            filter: None,
        }
    );
}

#[test]
fn a_window_freed_from_its_scope_names_its_table_in_a_single_relation_statement() {
    // The statement the expression is rendered into addresses `users` alone, which
    // is exactly the shape where a bare reference stays unqualified: without the
    // rewrite the window would read the relation the correlated statement selects
    // from under that same name instead of the one it was built over.
    let window = ExprKind::Window {
        kind: Box::new(WindowExprKind::Lead {
            column: Expr::column::<UserId>().into_kind(),
            offset: None,
        }),
        partition_by: Vec::new(),
        order_by: Vec::new(),
        frame: None,
    };

    let unscoped: Expr<i64, Unscoped> = Expr::from_kind(window).without_scope();

    let mut sql = String::new();
    let mut params = Vec::new();
    render_expr::<MockDialect>(
        &mut sql,
        &mut params,
        unscoped.kind(),
        &[crate::schema::TableRef::plain("users")],
    )
    .expect("a valid window expression");

    assert_eq!(sql, "LEAD(\"users\".\"id\") OVER ()");
    assert!(params.is_empty());
}

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
