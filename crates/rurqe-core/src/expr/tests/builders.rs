use super::*;

crate::table! {
    Order => "orders" {
        OrderTotal => "total": f64,
        OrderTax => "tax": f64,
        OrderQty => "qty": i64,
    }
}

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

    crate::table! {
        Order => "orders" {
            OrderId => "id": i64,
            OrderUserId => "user_id": i64,
        }
    }

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
#[should_panic(expected = "the aggregation marker names an aggregate call, but the tree is not one")]
fn a_forged_direct_aggregate_marker_cannot_silently_drop_a_filter() {
    // `Expr::new` stays crate-internal, so only this crate can assemble a tree
    // that disagrees with the aggregation marker it carries. The assertion
    // keeps the forged marker loud in debug builds instead of letting the
    // total fallback discard the predicate while the return type still
    // advertises the `FILTER` clause.
    let forged: Expr<i64, Order, DirectAggregate> = Expr::new(ExprKind::Column {
        table: "orders",
        name: "qty",
    });

    let _ = forged.filter_where(OrderQty.gt(0i64));
}

#[test]
#[should_panic(expected = "the aggregation marker names an aggregate call, but the tree is not one")]
fn a_forged_filtered_aggregate_marker_cannot_silently_ignore_distinct() {
    // The `DISTINCT` modifier mutates the aggregate call it belongs to, so a
    // forged marker on a non-aggregate tree would silently mutate nothing; the
    // assertion fires before the method returns an expression that still
    // carries the marker.
    let forged: Expr<i64, Order, FilteredAggregate> = Expr::new(ExprKind::Column {
        table: "orders",
        name: "qty",
    });

    let _ = forged.distinct();
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
        vec![
            "qty + ".to_owned(),
            " AND tax > ".to_owned(),
            String::new(),
        ],
        vec![SqlVal::I64(1), SqlVal::I64(10)],
    );

    let (sql, params) = render(expr.kind());

    assert_eq!(sql, "qty + ? AND tax > ?");
    assert_eq!(params, vec![SqlVal::I64(1), SqlVal::I64(10)]);
}
