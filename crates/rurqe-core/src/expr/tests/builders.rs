use super::*;

mod composition;
mod dialect;
mod fragments;
mod operands;
mod window;

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
            order_by: Vec::new(),
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
            order_by: Vec::new(),
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
            order_by: Vec::new(),
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
            order_by: Vec::new(),
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
            order_by: Vec::new(),
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
            order_by: Vec::new(),
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
            fragments: RawFragments::Static(&["1 = 0"]),
            params: Vec::new(),
        }
    );

    let everything = Expr::column::<UserId>().not_in_list(Vec::<i64>::new());
    assert_eq!(
        everything.kind(),
        &ExprKind::Raw {
            fragments: RawFragments::Static(&["1 = 1"]),
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
            fragments: RawFragments::Static(&["COUNT(*)"]),
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
    assert_eq!(UserId.gte(7i64), Expr::column::<UserId>().gte(7i64));
    assert_eq!(UserId.lte(7i64), Expr::column::<UserId>().lte(7i64));
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
fn inclusive_comparisons_render_the_inclusive_operators() {
    let lower = Expr::column::<UserId>().gte(10i64);
    let (sql, params) = render(lower.kind());
    assert_eq!(sql, "(\"id\" >= ?)");
    assert_eq!(params, vec![SqlVal::I64(10)]);

    let upper = Expr::column::<UserId>().lte(20i64);
    let (sql, params) = render(upper.kind());
    assert_eq!(sql, "(\"id\" <= ?)");
    assert_eq!(params, vec![SqlVal::I64(20)]);
}

#[test]
fn string_columns_match_like_patterns() {
    assert_eq!(UserName.like("A%"), Expr::column::<UserName>().like("A%"));
    assert_eq!(UserName.ilike("a%"), Expr::column::<UserName>().ilike("a%"));
}

#[test]
fn aggregate_helpers_build_expected_expressions() {
    assert_eq!(count(UserId), Expr::<i64>::count::<UserId>());
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
    let _: Expr<i64, User, DirectAggregate> = count(UserId);
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
            order_by: Vec::new(),
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
            order_by: Vec::new(),
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
fn unbalanced_raw_fragments_are_rejected() {
    // The renderer binds a parameter after every fragment except the last, so
    // a pair violating the interleaving invariant would dispatch SQL with a
    // missing or a misplaced placeholder; the mismatch is reported as a typed
    // error instead of dispatching broken SQL.
    let expression = Expr::<i64>::raw_fragments(&["qty > "], vec![SqlVal::I64(1), SqlVal::I64(10)]);

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
        &["qty + ", " AND tax > ", ""],
        vec![SqlVal::I64(1), SqlVal::I64(10)],
    );

    let (sql, params) = render(expr.kind());

    assert_eq!(sql, "qty + ? AND tax > ?");
    assert_eq!(params, vec![SqlVal::I64(1), SqlVal::I64(10)]);
}

#[test]
fn borrowed_and_owned_fragments_compare_by_their_text() {
    // The AST stays comparable by content: a template the macro split at compile
    // time and the same template split at run time describe one expression, so
    // the rules that match one expression against another keep observing the
    // value it names rather than the storage it chose.
    let borrowed = Expr::<i64>::raw_fragments(&["qty > ", ""], vec![SqlVal::I64(1)]);
    let owned = Expr::<i64>::raw_owned_fragments(
        vec!["qty > ".to_owned(), String::new()],
        vec![SqlVal::I64(1)],
    );

    assert_eq!(borrowed.kind(), owned.kind());
}

crate::table!(
    Note => "notes" {
        NoteId => "id": i64,
        NoteBody => "body": Option<String>,
        NoteScore => "score": Option<i64>,
    }
);

#[test]
fn a_nullable_column_compares_against_a_value_of_its_inner_type() {
    let equality = NoteBody.eq("draft");
    assert_eq!(equality, Expr::column::<NoteBody>().eq("draft"));
    let (sql, params) = render(equality.kind());
    assert_eq!(sql, "(\"body\" = ?)");
    assert_eq!(params, vec![SqlVal::Text("draft".to_owned())]);

    let inequality = NoteBody.ne("final");
    assert_eq!(inequality, Expr::column::<NoteBody>().ne("final"));
    let (sql, params) = render(inequality.kind());
    assert_eq!(sql, "(\"body\" <> ?)");
    assert_eq!(params, vec![SqlVal::Text("final".to_owned())]);

    let range = NoteScore.between(10i64, 20i64);
    assert_eq!(range, Expr::column::<NoteScore>().between(10i64, 20i64));
    let (sql, params) = render(range.kind());
    assert_eq!(sql, "(\"score\" BETWEEN ? AND ?)");
    assert_eq!(params, vec![SqlVal::I64(10), SqlVal::I64(20)]);

    let membership = NoteBody.in_list(vec!["draft", "final"]);
    assert_eq!(
        membership,
        Expr::column::<NoteBody>().in_list(vec!["draft", "final"])
    );
    let (sql, params) = render(membership.kind());
    assert_eq!(sql, "(\"body\" IN (?, ?))");
    assert_eq!(
        params,
        vec![
            SqlVal::Text("draft".to_owned()),
            SqlVal::Text("final".to_owned()),
        ]
    );
}

#[test]
fn a_nullable_column_tests_absence_with_the_null_predicates() {
    let absent = NoteBody.is_null();
    assert_eq!(absent, Expr::column::<NoteBody>().is_null());
    assert_eq!(
        absent.kind(),
        &ExprKind::IsNull {
            operand: Box::new(ExprKind::Column {
                table: "notes",
                name: "body",
            }),
            negated: false,
        }
    );
    let (sql, params) = render(absent.kind());
    assert_eq!(sql, "(\"body\" IS NULL)");
    assert!(params.is_empty());

    let present = NoteBody.is_not_null();
    assert_eq!(present, Expr::column::<NoteBody>().is_not_null());
    let (sql, params) = render(present.kind());
    assert_eq!(sql, "(\"body\" IS NOT NULL)");
    assert!(params.is_empty());
}
