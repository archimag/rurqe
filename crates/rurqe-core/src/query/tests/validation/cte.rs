//! The `WITH` clause and the relation namespace of its body.
//!
//! A `WITH` body is validated against the relations it declares itself, so a
//! reference it writes reaches no relation of the statement declaring it, and two
//! entries of one clause cannot share a name. The tests below pin both rules down.

use super::super::{MockDialect, Order, OrderId, User, UserId, UserName};
use super::{qualified_body, relation_statement};
use crate::expr::CteAst;
use crate::query::*;

#[test]
fn a_duplicate_named_cte_is_rejected_before_rendering() {
    let first = SelectQuery::<MockDialect>::new()
        .from::<User>()
        .select((UserId, UserName));
    let second = SelectQuery::<MockDialect>::new()
        .from::<User>()
        .select((UserId,));

    let query = SelectQuery::<MockDialect>::new()
        .with(cte_named("active_users", first))
        .with(cte_named("active_users", second))
        .from::<User>()
        .select((UserId,));

    assert_eq!(
        query.build(),
        Err(BuildError::DuplicateCte {
            name: "active_users"
        })
    );
}

#[test]
fn two_differently_named_ctes_are_accepted() {
    let active = SelectQuery::<MockDialect>::new()
        .from::<User>()
        .select((UserId, UserName));
    let large = SelectQuery::<MockDialect>::new()
        .from::<Order>()
        .select((OrderId,));

    let built = SelectQuery::<MockDialect>::new()
        .with(cte_named("active_users", active))
        .with(cte_named("large_orders", large))
        .from::<User>()
        .select((UserId,))
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "WITH \"active_users\" AS (SELECT \"id\", \"name\" FROM \"users\"), \
         \"large_orders\" AS (SELECT \"id\" FROM \"orders\") SELECT \"id\" FROM \"users\""
    );
}

#[test]
fn build_error_display_duplicate_cte() {
    let err = BuildError::DuplicateCte {
        name: "active_users",
    };
    let msg = err.to_string();
    assert!(msg.contains("duplicate CTE name `active_users`"));
    assert!(msg.contains("cte_named"));
}

/// Declares a statement projecting one column as a Common Table Expression named `name`.
fn cte(name: &'static str) -> CteAst {
    CteAst {
        name,
        query: relation_statement("orders", "id"),
    }
}

#[test]
fn a_statement_declaring_one_cte_name_twice_is_rejected() {
    let mut ast = relation_statement("orders", "id");
    ast.ctes.push(cte("active_users"));
    ast.ctes.push(cte("active_users"));

    assert_eq!(
        ast.validate(),
        Err(BuildError::DuplicateCte {
            name: "active_users"
        })
    );
}

#[test]
fn a_statement_declaring_differently_named_ctes_is_accepted() {
    let mut ast = relation_statement("orders", "id");
    ast.ctes.push(cte("active_users"));
    ast.ctes.push(cte("large_orders"));

    assert_eq!(ast.validate(), Ok(()));
}

#[test]
fn a_common_table_expression_resolves_its_own_relations() {
    let mut ast = relation_statement("orders", "id");
    ast.ctes.push(CteAst {
        name: "active_users",
        query: relation_statement("users", "id"),
    });

    assert_eq!(ast.validate(), Ok(()));
}

#[test]
fn a_common_table_expression_correlates_with_no_enclosing_statement() {
    // The `WITH` body is validated against its own relations alone: the engine
    // correlates none of it to the statement declaring it.
    let mut ast = relation_statement("orders", "id");
    ast.ctes.push(CteAst {
        name: "active_users",
        query: qualified_body("users"),
    });

    assert_eq!(
        ast.validate(),
        Err(BuildError::UnresolvedQualifier { name: "users" })
    );
}
