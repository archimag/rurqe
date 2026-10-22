use super::*;

#[test]
fn column_expression_carries_static_name() {
    assert_eq!(
        Expr::column::<UserId>().kind(),
        &ExprKind::Column { name: "id" }
    );
    assert_eq!(
        Expr::column::<UserName>().kind(),
        &ExprKind::Column { name: "name" }
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
            lhs: Box::new(ExprKind::Column { name: "id" }),
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
            lhs: Box::new(ExprKind::Column { name: "name" }),
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
            lhs: Box::new(ExprKind::Column { name: "name" }),
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
            arg: None,
        }
    );
    assert_eq!(
        Expr::count::<UserId>().kind(),
        &ExprKind::Aggregate {
            func: AggregateFunc::Count,
            arg: Some(Box::new(ExprKind::Column { name: "id" })),
        }
    );
    assert_eq!(
        Expr::sum::<UserId>().kind(),
        &ExprKind::Aggregate {
            func: AggregateFunc::Sum,
            arg: Some(Box::new(ExprKind::Column { name: "id" })),
        }
    );
    assert_eq!(
        Expr::avg::<UserId>().kind(),
        &ExprKind::Aggregate {
            func: AggregateFunc::Avg,
            arg: Some(Box::new(ExprKind::Column { name: "id" })),
        }
    );
    assert_eq!(
        Expr::min::<UserId>().kind(),
        &ExprKind::Aggregate {
            func: AggregateFunc::Min,
            arg: Some(Box::new(ExprKind::Column { name: "id" })),
        }
    );
    assert_eq!(
        Expr::max::<UserName>().kind(),
        &ExprKind::Aggregate {
            func: AggregateFunc::Max,
            arg: Some(Box::new(ExprKind::Column { name: "name" })),
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
            lhs: Box::new(ExprKind::Column { name: "id" }),
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
            lhs: Box::new(ExprKind::Column { name: "id" }),
            rhs: Box::new(ExprKind::Column { name: "id" }),
        }
    );
}

#[test]
fn like_and_ilike_build_pattern_predicates() {
    let predicate = Expr::column::<UserName>().like("A%");
    assert_eq!(
        predicate.kind(),
        &ExprKind::Like {
            operand: Box::new(ExprKind::Column { name: "name" }),
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
    assert_eq!(
        UserName.ilike("a%"),
        Expr::column::<UserName>().ilike("a%")
    );
}

#[test]
fn aggregate_helpers_build_expected_expressions() {
    assert_eq!(count(), Expr::<i64>::count_all());
    assert_eq!(count_all(), Expr::<i64>::count_all());
    assert_eq!(sum(UserId), Expr::<i64>::sum::<UserId>());
    assert_eq!(avg(UserId), Expr::<f64>::avg::<UserId>());
    assert_eq!(min(UserId), Expr::<i64>::min::<UserId>());
    assert_eq!(max(UserName), Expr::<String>::max::<UserName>());
}

#[test]
fn expressions_carry_table_scopes_across_operators() {
    // Single-table comparison carries the table scope
    let eq_expr: Expr<bool, User> = Expr::column::<UserId>().eq(42i64);
    assert_eq!(
        eq_expr.kind(),
        &ExprKind::Binary {
            op: BinaryOp::Eq,
            lhs: Box::new(ExprKind::Column { name: "id" }),
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
    assert!(matches!(cross_eq.kind(), ExprKind::Binary { op: BinaryOp::Eq, .. }));

    // and / or preserve and merge table scopes
    let and_expr: Expr<bool, (User, Order)> = UserId.eq(1i64).and(OrderUserId.eq(2i64));
    assert!(matches!(and_expr.kind(), ExprKind::Binary { op: BinaryOp::And, .. }));

    let or_expr: Expr<bool, (User, Order)> = UserId.eq(1i64).or(OrderUserId.eq(2i64));
    assert!(matches!(or_expr.kind(), ExprKind::Binary { op: BinaryOp::Or, .. }));

    // between preserves table scope
    let between_expr: Expr<bool, User> = UserId.between(1i64, 10i64);
    assert!(matches!(between_expr.kind(), ExprKind::Between { .. }));

    // like and ilike preserve table scope
    let like_expr: Expr<bool, User> = UserName.like("alice%");
    assert!(matches!(like_expr.kind(), ExprKind::Like { case_insensitive: false, .. }));

    let ilike_expr: Expr<bool, User> = UserName.ilike("alice%");
    assert!(matches!(ilike_expr.kind(), ExprKind::Like { case_insensitive: true, .. }));

    // is_null and is_not_null preserve table scope
    let null_expr: Expr<bool, User> = UserName.is_null();
    assert!(matches!(null_expr.kind(), ExprKind::IsNull { negated: false, .. }));

    let not_null_expr: Expr<bool, User> = UserName.is_not_null();
    assert!(matches!(not_null_expr.kind(), ExprKind::IsNull { negated: true, .. }));
}
