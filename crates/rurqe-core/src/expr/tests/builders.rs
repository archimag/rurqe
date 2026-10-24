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
    let _: Expr<i64, User, Aggregate> = count_distinct(UserId);
    let _: Expr<i64, User, Aggregate> = sum(UserId);
    let _: Expr<i64, User, Aggregate> = sum_distinct(UserId);
    let _: Expr<f64, User, Aggregate> = avg(UserId);
    let _: Expr<f64, User, Aggregate> = avg_distinct(UserId);
    let _: Expr<i64, User, Aggregate> = min(UserId);
    let _: Expr<String, User, Aggregate> = max(UserName);

    // The constructors of `Expr` keep the same scope as the free functions.
    let _: Expr<i64, User, Aggregate> = Expr::<i64>::count::<UserId>();
    let _: Expr<i64, User, Aggregate> = Expr::<i64>::count_distinct::<UserId>();
    let _: Expr<i64, User, Aggregate> = Expr::<i64>::sum::<UserId>();
    let _: Expr<i64, User, Aggregate> = Expr::<i64>::sum_distinct::<UserId>();
    let _: Expr<f64, User, Aggregate> = Expr::<f64>::avg::<UserId>();
    let _: Expr<f64, User, Aggregate> = Expr::<f64>::avg_distinct::<UserId>();
    let _: Expr<i64, User, Aggregate> = Expr::<i64>::min::<UserId>();
    let _: Expr<String, User, Aggregate> = Expr::<String>::max::<UserName>();

    // `COUNT(*)` reads no column, so it references no table and stays constant
    // within every group.
    let _: Expr<i64, (), Aggregate> = count_all();
    let _: Expr<i64, (), Aggregate> = count();
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
    // The aggregate references `orders`, and the bound operand adds no table.
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
fn arithmetic_preserves_a_filtered_aggregate_next_to_a_constant() {
    let filtered = sum(OrderTotal).filter_where(OrderQty.gt(0i64));

    // A constant operand is the same in every row of the group, so the scaled
    // filtered aggregate stays projectable from a grouped query.
    let scaled: Expr<f64, _, FilteredAggregate> = filtered * 2.0f64;
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
