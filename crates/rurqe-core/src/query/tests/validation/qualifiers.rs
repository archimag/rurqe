//! Resolution of a qualifier against the relations of the statement.
//!
//! A reference qualified through a relation name reaches the relation the statement
//! renders under that name, or one the statement encloses it in by correlation. The
//! tests below pin the accepted spellings down beside the exact diagnostic a
//! qualifier nothing declares reports.

use super::super::{MockDialect, Order, OrderAlias, OrderId, RecentId, RecentOrder};
use super::{empty_ast, exists_subquery, relation_statement, scalar_subquery};
use crate::expr::{
    AggregateFunc, BinaryOp, ExprKind, JoinAst, JoinKind, OrderCriterion, OrderDirection, SelectAst,
    SqlVal, Subquery, SubqueryOp,
};
use crate::query::validate::reject_unresolved_qualifiers;
use crate::query::*;
use crate::schema::{Aliased, AliasedCol, TableRef};

#[test]
fn a_projection_qualified_by_the_declared_alias_is_accepted() {
    let built = SelectQuery::<MockDialect>::new()
        .from::<Aliased<Order, OrderAlias>>()
        .select((AliasedCol::<OrderAlias, OrderId>::new(),))
        .build()
        .expect("valid query");

    assert_eq!(built.sql, "SELECT \"o\".\"id\" FROM \"orders\" AS \"o\"");
}

#[test]
fn a_projection_qualified_by_a_name_the_statement_does_not_declare_is_rejected() {
    let mut ast = SelectQuery::<MockDialect>::new()
        .from::<Order>()
        .select((OrderId,))
        .to_ast();
    ast.projections = vec![ExprKind::QualifiedColumn {
        table: "o",
        column: "id",
    }];

    assert_eq!(
        ast.validate(),
        Err(BuildError::UnresolvedQualifier { name: "o" })
    );
}

#[test]
fn a_filter_qualified_by_a_name_the_statement_does_not_declare_is_rejected() {
    let mut ast = SelectQuery::<MockDialect>::new()
        .from::<Order>()
        .select((OrderId,))
        .to_ast();
    ast.filter = Some(ExprKind::Binary {
        op: BinaryOp::Eq,
        lhs: Box::new(ExprKind::QualifiedColumn {
            table: "o",
            column: "user_id",
        }),
        rhs: Box::new(ExprKind::Column {
            table: "orders",
            name: "id",
        }),
    });

    assert_eq!(
        ast.validate(),
        Err(BuildError::UnresolvedQualifier { name: "o" })
    );
}

#[test]
fn an_order_by_qualified_by_a_name_the_statement_does_not_declare_is_rejected() {
    let mut ast = SelectQuery::<MockDialect>::new()
        .from::<Order>()
        .select((OrderId,))
        .to_ast();
    ast.order_by.push(OrderCriterion::new(
        ExprKind::QualifiedColumn {
            table: "o",
            column: "total",
        },
        OrderDirection::Asc,
    ));

    assert_eq!(
        ast.validate(),
        Err(BuildError::UnresolvedQualifier { name: "o" })
    );
}

/// Builds a statement whose filter is a subquery over `recent` correlating a
/// reference qualified as `qualifier` to the enclosing `orders AS o` statement.
fn correlated_statement(qualifier: &'static str) -> SelectAst {
    let mut inner = SelectQuery::<MockDialect>::new()
        .from::<RecentOrder>()
        .select((RecentId,))
        .to_ast();
    inner.filter = Some(ExprKind::Binary {
        op: BinaryOp::Eq,
        lhs: Box::new(ExprKind::Column {
            table: "recent",
            name: "id",
        }),
        rhs: Box::new(ExprKind::QualifiedColumn {
            table: qualifier,
            column: "id",
        }),
    });

    let mut outer = SelectQuery::<MockDialect>::new()
        .from::<Aliased<Order, OrderAlias>>()
        .select((AliasedCol::<OrderAlias, OrderId>::new(),))
        .to_ast();
    outer.filter = Some(ExprKind::Subquery {
        op: SubqueryOp::Exists,
        operand: None,
        query: Subquery::new(inner),
        negated: false,
    });
    outer
}

#[test]
fn a_correlated_subquery_may_reference_the_alias_of_the_enclosing_statement() {
    // The nested statement introduces `recent` only, so `o.id` resolves through the
    // alias the statement enclosing it declares.
    assert_eq!(correlated_statement("o").validate(), Ok(()));
}

#[test]
fn a_correlated_subquery_qualified_by_a_name_nothing_declares_is_rejected() {
    assert_eq!(
        correlated_statement("ghosts").validate(),
        Err(BuildError::UnresolvedQualifier { name: "ghosts" })
    );
}

#[test]
fn a_join_on_predicate_qualified_by_a_name_nothing_declares_is_rejected() {
    // The `ON` condition of a join belongs to the statement scope, so it resolves its
    // references against the relations the statement introduces.
    let mut ast = relation_statement("users", "id");
    ast.joins.push(JoinAst {
        kind: JoinKind::Inner,
        table: TableRef::plain("orders"),
        lateral: None,
        on: Some(ExprKind::Binary {
            op: BinaryOp::Eq,
            lhs: Box::new(ExprKind::Column {
                table: "orders",
                name: "user_id",
            }),
            rhs: Box::new(ExprKind::QualifiedColumn {
                table: "ghosts",
                column: "id",
            }),
        }),
    });

    assert_eq!(
        ast.validate(),
        Err(BuildError::UnresolvedQualifier { name: "ghosts" })
    );
}

#[test]
fn a_group_by_expression_qualified_by_a_name_nothing_declares_is_rejected() {
    // The `GROUP BY` entry is a section of the statement like the projection list,
    // so a qualifier nothing declares is rejected from it as well. The projection
    // stays a literal so the grouped entry is the only reference the walk resolves.
    let mut ast = relation_statement("orders", "user_id");
    ast.projections = vec![ExprKind::Value(SqlVal::I64(1))];
    ast.group_by.push(ExprKind::QualifiedColumn {
        table: "ghosts",
        column: "id",
    });

    assert_eq!(
        ast.validate(),
        Err(BuildError::UnresolvedQualifier { name: "ghosts" })
    );
}

#[test]
fn a_distinct_on_expression_qualified_by_a_name_nothing_declares_is_rejected() {
    let mut ast = relation_statement("orders", "id");
    ast.distinct_on.push(ExprKind::QualifiedColumn {
        table: "ghosts",
        column: "id",
    });

    assert_eq!(
        ast.validate(),
        Err(BuildError::UnresolvedQualifier { name: "ghosts" })
    );
}

#[test]
fn a_having_predicate_qualified_by_a_name_nothing_declares_is_rejected() {
    // The aggregate reads the foreign reference, so the aggregation consistency
    // leaves the clause alone and the qualifier rule is the one that reports it.
    let mut ast = relation_statement("orders", "user_id");
    ast.group_by.push(ExprKind::Column {
        table: "orders",
        name: "user_id",
    });
    ast.having = Some(ExprKind::Binary {
        op: BinaryOp::Gt,
        lhs: Box::new(ExprKind::Aggregate {
            func: AggregateFunc::Count,
            distinct: false,
            arg: Some(Box::new(ExprKind::QualifiedColumn {
                table: "ghosts",
                column: "id",
            })),
            filter: None,
        }),
        rhs: Box::new(ExprKind::Value(SqlVal::I64(0))),
    });

    assert_eq!(
        ast.validate(),
        Err(BuildError::UnresolvedQualifier { name: "ghosts" })
    );
}

#[test]
fn a_doubly_nested_subquery_reaches_the_outermost_relation_by_correlation() {
    // The enclosing namespace accumulates statement by statement, so a reference
    // nested two levels deep still resolves against the outermost relation.
    let innermost = super::qualified_body("users");

    let mut middle = relation_statement("orders", "id");
    middle.projections.push(scalar_subquery(innermost));

    let mut ast = relation_statement("users", "id");
    ast.filter = Some(exists_subquery(middle));

    assert_eq!(ast.validate(), Ok(()));
}

#[test]
fn a_doubly_nested_subquery_qualified_by_a_name_nothing_declares_is_rejected() {
    let innermost = super::qualified_body("ghosts");

    let mut middle = relation_statement("orders", "id");
    middle.projections.push(scalar_subquery(innermost));

    let mut ast = relation_statement("users", "id");
    ast.filter = Some(exists_subquery(middle));

    assert_eq!(
        ast.validate(),
        Err(BuildError::UnresolvedQualifier { name: "ghosts" })
    );
}

#[test]
fn build_error_display_unresolved_qualifier() {
    let err = BuildError::UnresolvedQualifier { name: "u" };
    let msg = err.to_string();
    assert!(msg.contains("qualified as `u`"));
    assert!(msg.contains("`Table::table_ref`"));
}

#[test]
fn a_reference_qualified_by_a_declared_relation_name_is_accepted() {
    let relations = [TableRef::aliased("users", "u"), TableRef::plain("accounts")];

    // The alias of the target and the plain name of a `FROM` source are both
    // declared by the rendered statement.
    assert_eq!(
        reject_unresolved_qualifiers(
            &ExprKind::QualifiedColumn {
                table: "u",
                column: "id",
            },
            &relations,
            &[],
        ),
        Ok(())
    );
    assert_eq!(
        reject_unresolved_qualifiers(
            &ExprKind::QualifiedColumn {
                table: "accounts",
                column: "id",
            },
            &relations,
            &[],
        ),
        Ok(())
    );

    // A plain column reference states no qualifier of its own: the renderer resolves
    // the qualifier it needs against the declared relations.
    assert_eq!(
        reject_unresolved_qualifiers(
            &ExprKind::Column {
                table: "users",
                name: "id",
            },
            &relations,
            &[],
        ),
        Ok(())
    );

    // The walk reaches a qualifier nested inside a composite expression.
    let filter = ExprKind::Binary {
        op: BinaryOp::Eq,
        lhs: Box::new(ExprKind::QualifiedColumn {
            table: "ghosts",
            column: "id",
        }),
        rhs: Box::new(ExprKind::Column {
            table: "users",
            name: "id",
        }),
    };
    assert_eq!(
        reject_unresolved_qualifiers(&filter, &relations, &[]),
        Err(BuildError::UnresolvedQualifier { name: "ghosts" })
    );
}

#[test]
fn a_reference_qualified_by_the_base_name_of_an_aliased_relation_is_rejected() {
    let relations = [TableRef::aliased("users", "u")];

    // The relation is only reachable as `u`, so a reference spelled through its base
    // table name resolves to nothing the statement renders.
    assert_eq!(
        reject_unresolved_qualifiers(
            &ExprKind::QualifiedColumn {
                table: "users",
                column: "id",
            },
            &relations,
            &[],
        ),
        Err(BuildError::UnresolvedQualifier { name: "users" })
    );
}

#[test]
fn a_correlated_subquery_may_reference_a_relation_of_the_enclosing_statement() {
    // The nested statement reads `u.id` without introducing the relation: a
    // correlated reference reaches the enclosing statement, whose relations are in
    // scope where the subquery is written.
    let mut inner = empty_ast();
    inner.from = Some(TableRef::plain("orders"));
    inner.projections.push(ExprKind::QualifiedColumn {
        table: "u",
        column: "id",
    });
    inner.filter = Some(ExprKind::Binary {
        op: BinaryOp::Eq,
        lhs: Box::new(ExprKind::QualifiedColumn {
            table: "u",
            column: "id",
        }),
        rhs: Box::new(ExprKind::Column {
            table: "orders",
            name: "user_id",
        }),
    });

    assert_eq!(
        reject_unresolved_qualifiers(
            &scalar_subquery(inner),
            &[TableRef::aliased("users", "u")],
            &[],
        ),
        Ok(())
    );
}

#[test]
fn an_outer_relation_stays_reachable_from_a_nested_statement() {
    // The relation belongs to a statement enclosing the validated one, so it is only
    // reachable through the outer scope the walk carries along.
    let mut inner = empty_ast();
    inner.projections.push(ExprKind::QualifiedColumn {
        table: "u",
        column: "id",
    });

    assert_eq!(
        reject_unresolved_qualifiers(
            &scalar_subquery(inner),
            &[],
            &[TableRef::aliased("users", "u")],
        ),
        Ok(())
    );
}

/// Builds a statement whose filter is a subquery over `recent` correlating a plain
/// column reference to the statement enclosing it.
///
/// The reference names `reference_table` explicitly, while `aliased` selects how
/// the enclosing statement addresses `orders`: under the `o` alias, or under the
/// table name it is declared by. The pair decides whether the qualifier the nested
/// statement renders is one the enclosing statement declares.
fn correlated_plain_statement(reference_table: &'static str, aliased: bool) -> SelectAst {
    let mut inner = SelectQuery::<MockDialect>::new()
        .from::<RecentOrder>()
        .select((RecentId,))
        .to_ast();
    inner.filter = Some(ExprKind::Binary {
        op: BinaryOp::Eq,
        lhs: Box::new(ExprKind::Column {
            table: "recent",
            name: "id",
        }),
        rhs: Box::new(ExprKind::Column {
            table: reference_table,
            name: "id",
        }),
    });

    let mut outer = if aliased {
        SelectQuery::<MockDialect>::new()
            .from::<Aliased<Order, OrderAlias>>()
            .select((AliasedCol::<OrderAlias, OrderId>::new(),))
            .to_ast()
    } else {
        SelectQuery::<MockDialect>::new()
            .from::<Order>()
            .select((OrderId,))
            .to_ast()
    };
    outer.filter = Some(ExprKind::Subquery {
        op: SubqueryOp::Exists,
        operand: None,
        query: Subquery::new(inner),
        negated: false,
    });
    outer
}

#[test]
fn a_correlated_plain_reference_to_an_unaliased_enclosing_relation_is_accepted() {
    // The enclosing statement renders `orders` under its own table name, so the
    // qualifier the nested statement writes resolves verbatim.
    assert_eq!(correlated_plain_statement("orders", false).validate(), Ok(()));
}

#[test]
fn a_correlated_plain_reference_to_the_base_name_of_an_aliased_enclosing_relation_is_rejected() {
    // The enclosing statement renders `FROM "orders" AS "o"`, so the `"orders"`
    // the nested statement writes names a relation that statement never declares —
    // what PostgreSQL answers with `missing FROM-clause entry for table "orders"`.
    // The renderer qualifies a correlated reference verbatim and substitutes no
    // alias across a statement boundary, so the reference is rejected before it
    // renders.
    assert_eq!(
        correlated_plain_statement("orders", true).validate(),
        Err(BuildError::UnresolvedColumn {
            table: "orders",
            column: "id",
        })
    );
}

#[test]
fn a_correlated_plain_reference_naming_the_alias_of_an_enclosing_relation_is_accepted() {
    // The enclosing statement renders the relation as `"orders" AS "o"`, and the
    // reference carries exactly that name.
    assert_eq!(correlated_plain_statement("o", true).validate(), Ok(()));
}

#[test]
fn a_correlated_plain_reference_to_the_base_name_of_an_outer_relation_is_rejected() {
    // The relation in scope belongs to a statement enclosing the validated one, and
    // that statement renders it under the alias alone, so the base name the
    // reference carries addresses no rendered relation.
    let mut inner = empty_ast();
    inner.projections.push(ExprKind::Column {
        table: "users",
        name: "id",
    });

    assert_eq!(
        reject_unresolved_qualifiers(
            &scalar_subquery(inner),
            &[],
            &[TableRef::aliased("users", "u")],
        ),
        Err(BuildError::UnresolvedColumn {
            table: "users",
            column: "id",
        })
    );
}

#[test]
fn a_correlated_plain_reference_naming_the_alias_of_an_outer_relation_is_accepted() {
    let mut inner = empty_ast();
    inner.projections.push(ExprKind::Column {
        table: "u",
        name: "id",
    });

    assert_eq!(
        reject_unresolved_qualifiers(
            &scalar_subquery(inner),
            &[],
            &[TableRef::aliased("users", "u")],
        ),
        Ok(())
    );
}

#[test]
fn a_nested_statement_qualified_by_a_name_nothing_declares_is_rejected() {
    let mut inner = empty_ast();
    inner.from = Some(TableRef::plain("orders"));
    inner.projections.push(ExprKind::QualifiedColumn {
        table: "ghosts",
        column: "id",
    });

    assert_eq!(
        reject_unresolved_qualifiers(
            &scalar_subquery(inner),
            &[TableRef::aliased("users", "u")],
            &[],
        ),
        Err(BuildError::UnresolvedQualifier { name: "ghosts" })
    );
}

#[test]
fn a_nested_collection_resolves_a_correlated_reference() {
    let mut inner = empty_ast();
    inner.from = Some(TableRef::plain("orders"));
    inner.projections.push(ExprKind::Column {
        table: "orders",
        name: "id",
    });
    inner.filter = Some(ExprKind::Binary {
        op: BinaryOp::Eq,
        lhs: Box::new(ExprKind::QualifiedColumn {
            table: "u",
            column: "id",
        }),
        rhs: Box::new(ExprKind::Column {
            table: "orders",
            name: "user_id",
        }),
    });

    let collection = ExprKind::Multiset {
        query: Subquery::new(inner),
        fields: vec!["id"],
        single: false,
    };

    assert_eq!(
        reject_unresolved_qualifiers(&collection, &[TableRef::aliased("users", "u")], &[]),
        Ok(())
    );
}
