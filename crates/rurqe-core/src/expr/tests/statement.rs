//! Unit tests for the dialect-neutral statement AST and window rendering.

use super::*;

use crate::schema::{Column, TableRef};

crate::table! {
    Order => "orders" {
        OrderId => "id": i64,
        OrderUserId => "user_id": i64,
    }
}

/// Builds a statement selecting `projections` from the `users` table.
fn select(projections: Vec<ExprKind>) -> SelectAst {
    SelectAst {
        ctes: Vec::new(),
        distinct: false,
        distinct_on: Vec::new(),
        projections,
        from: Some(TableRef::plain("users")),
        joins: Vec::new(),
        filter: None,
        group_by: Vec::new(),
        having: None,
        order_by: Vec::new(),
        limit: None,
        offset: None,
        set_operations: Vec::new(),
        compound_order_by: Vec::new(),
        compound_limit: None,
        compound_offset: None,
    }
}

/// Renders `ast` with the mock dialect into its SQL text and parameters.
fn render_statement(ast: &SelectAst) -> (String, Vec<SqlVal>) {
    let mut sql = String::new();
    let mut params = Vec::new();
    ast.render::<MockDialect>(&mut sql, &mut params);
    (sql, params)
}

#[test]
fn a_statement_renders_every_clause_in_sql_order() {
    let mut ast = select(vec![UserId::expr_kind(), UserName::expr_kind()]);
    ast.filter = Some(Expr::column::<UserId>().gt(10i64).into_kind());
    ast.order_by = vec![(UserId::expr_kind(), OrderDirection::Desc)];
    ast.limit = Some(5);
    ast.offset = Some(2);

    let (sql, params) = render_statement(&ast);

    assert_eq!(
        sql,
        "SELECT \"id\", \"name\" FROM \"users\" WHERE (\"id\" > ?) \
         ORDER BY \"id\" DESC LIMIT 5 OFFSET 2"
    );
    assert_eq!(params, vec![SqlVal::I64(10)]);
}

#[test]
fn common_table_expressions_render_before_the_projection() {
    let mut ast = select(vec![UserId::expr_kind()]);
    ast.ctes = vec![CteAst {
        name: "active_users",
        query: select(vec![UserId::expr_kind()]),
    }];

    let (sql, _) = render_statement(&ast);

    assert_eq!(
        sql,
        "WITH \"active_users\" AS (SELECT \"id\" FROM \"users\") \
         SELECT \"id\" FROM \"users\""
    );
}

#[test]
fn joins_render_their_kind_relation_and_condition() {
    let mut ast = select(vec![UserId::expr_kind(), OrderId::expr_kind()]);
    ast.joins = vec![JoinAst {
        kind: JoinKind::Left,
        table: TableRef::plain("orders"),
        lateral: None,
        on: Some(Expr::column::<OrderUserId>().eq(1i64).into_kind()),
    }];

    let (sql, params) = render_statement(&ast);

    assert_eq!(
        sql,
        "SELECT \"users\".\"id\", \"orders\".\"id\" FROM \"users\" LEFT JOIN \"orders\" \
         ON (\"orders\".\"user_id\" = ?)"
    );
    assert_eq!(params, vec![SqlVal::I64(1)]);
}

#[test]
fn a_union_of_plain_statements_needs_no_parentheses() {
    let mut ast = select(vec![UserId::expr_kind()]);
    ast.set_operations = vec![SetOpAst {
        kind: SetOpKind::Union,
        query: select(vec![UserId::expr_kind()]),
    }];
    ast.compound_order_by = vec![(UserId::expr_kind(), OrderDirection::Asc)];
    ast.compound_limit = Some(10);

    let (sql, _) = render_statement(&ast);

    assert_eq!(
        sql,
        "SELECT \"id\" FROM \"users\" UNION SELECT \"id\" FROM \"users\" \
         ORDER BY \"id\" ASC LIMIT 10"
    );
}

#[test]
fn operands_carrying_their_own_clauses_are_parenthesised() {
    let mut right = select(vec![UserId::expr_kind()]);
    right.order_by = vec![(UserId::expr_kind(), OrderDirection::Desc)];

    let mut left = select(vec![UserId::expr_kind()]);
    left.limit = Some(1);
    left.set_operations = vec![SetOpAst {
        kind: SetOpKind::UnionAll,
        query: right,
    }];

    let (sql, _) = render_statement(&left);

    assert_eq!(
        sql,
        "(SELECT \"id\" FROM \"users\" LIMIT 1) UNION ALL \
         (SELECT \"id\" FROM \"users\" ORDER BY \"id\" DESC)"
    );
}

#[test]
fn a_nested_set_operation_is_parenthesised() {
    let mut inner = select(vec![UserId::expr_kind()]);
    inner.set_operations = vec![SetOpAst {
        kind: SetOpKind::Union,
        query: select(vec![UserId::expr_kind()]),
    }];

    let mut outer = select(vec![UserId::expr_kind()]);
    outer.set_operations = vec![SetOpAst {
        kind: SetOpKind::Except,
        query: inner,
    }];

    let (sql, _) = render_statement(&outer);

    assert_eq!(
        sql,
        "SELECT \"id\" FROM \"users\" EXCEPT \
         (SELECT \"id\" FROM \"users\" UNION SELECT \"id\" FROM \"users\")"
    );
}

#[test]
fn a_window_expression_renders_its_partition_and_ordering() {
    let window = ExprKind::Window {
        kind: Box::new(WindowExprKind::RowNumber),
        partition_by: vec![UserId::expr_kind()],
        order_by: vec![(UserId::expr_kind(), OrderDirection::Desc)],
    };

    let (sql, params) = render(&window);

    assert_eq!(
        sql,
        "ROW_NUMBER() OVER (PARTITION BY \"id\" ORDER BY \"id\" DESC)"
    );
    assert!(params.is_empty());
}

#[test]
fn lag_and_lead_render_their_offsets() {
    let lag = ExprKind::Window {
        kind: Box::new(WindowExprKind::Lag {
            column: UserId::expr_kind(),
            offset: Some(2),
        }),
        partition_by: Vec::new(),
        order_by: Vec::new(),
    };
    assert_eq!(render(&lag).0, "LAG(\"id\", 2) OVER ()");

    let lead = ExprKind::Window {
        kind: Box::new(WindowExprKind::Lead {
            column: UserId::expr_kind(),
            offset: None,
        }),
        partition_by: Vec::new(),
        order_by: vec![(UserId::expr_kind(), OrderDirection::Asc)],
    };
    assert_eq!(render(&lead).0, "LEAD(\"id\") OVER (ORDER BY \"id\" ASC)");
}

#[test]
fn an_aggregate_evaluated_over_a_window_keeps_its_argument() {
    let summed = ExprKind::Window {
        kind: Box::new(WindowExprKind::Expr(ExprKind::Aggregate {
            func: AggregateFunc::Sum,
            arg: Some(Box::new(UserId::expr_kind())),
        })),
        partition_by: Vec::new(),
        order_by: Vec::new(),
    };

    let (sql, params) = render(&summed);

    assert_eq!(sql, "SUM(\"id\") OVER ()");
    assert!(params.is_empty());
}

/// A dialect distinct from the mock one, used to check dialect markers.
struct OtherDialect;

impl crate::dialect::Dialect for OtherDialect {
    fn push_quoted_identifier(out: &mut String, ident: &str) {
        out.push('[');
        out.push_str(ident);
        out.push(']');
    }

    fn push_placeholder(out: &mut String, _index: usize) {
        out.push('?');
    }
}

#[test]
fn dialect_markers_merge_around_the_neutral_marker() {
    fn assert_merged<L: MergeDialect<R, Output = Expected>, R, Expected>() {}

    assert_merged::<(), (), ()>();
    assert_merged::<MockDialect, (), MockDialect>();
    assert_merged::<(), MockDialect, MockDialect>();
    assert_merged::<MockDialect, MockDialect, MockDialect>();
}

#[test]
fn the_dialect_alias_names_the_merged_marker() {
    fn assert_alias<L, R, Expected>()
    where
        L: MergeDialect<R, Output = Expected>,
    {
        let _: core::marker::PhantomData<MergedDialect<L, R>> = core::marker::PhantomData;
    }

    assert_alias::<MockDialect, (), MockDialect>();
    assert_alias::<(), MockDialect, MockDialect>();
}

#[test]
fn dialect_neutral_nodes_render_under_every_dialect() {
    fn assert_in_scope<D, Node: DialectInScope<D>>() {}

    assert_in_scope::<MockDialect, ()>();
    assert_in_scope::<OtherDialect, ()>();
    assert_in_scope::<MockDialect, MockDialect>();
}
