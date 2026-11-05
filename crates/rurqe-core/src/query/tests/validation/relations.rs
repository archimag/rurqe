//! Relation membership of a column reference.
//!
//! A statement declares the relations it renders through its `FROM` clause, its
//! joins, and its derived tables, and a reference has to name one of them — or
//! reach one of them through an enclosing statement by correlation. The tests below
//! pin the registration rules down beside the exact diagnostics a reference outside
//! the declared namespace reports.

use super::{empty_ast, exists_subquery, relation_statement, scalar_subquery};
use crate::expr::{BinaryOp, DerivedAst, Expr, ExprKind, SelectAst, SqlVal};
use crate::query::validate::{register_relation, reject_shadowed_derived};
use crate::query::*;
use crate::schema::TableRef;

#[test]
fn build_error_display_duplicate_relation() {
    let err = BuildError::DuplicateRelation { name: "users" };
    let msg = err.to_string();
    assert!(msg.contains("duplicate unaliased relation `users`"));
    assert!(msg.contains("table_alias!"));
}

#[test]
fn build_error_display_aliased_derived_relation() {
    let err = BuildError::AliasedDerivedRelation { name: "o" };
    let msg = err.to_string();
    assert!(msg.contains("addressed as `o`"));
    assert!(msg.contains("registered under another name"));
}

/// Registers a derived subquery projecting one column under `name`.
fn derived(name: &'static str) -> DerivedAst {
    DerivedAst {
        name,
        query: relation_statement("orders", "id"),
    }
}

#[test]
fn a_derived_relation_registered_under_the_base_name_of_its_alias_is_rejected() {
    assert_eq!(
        reject_shadowed_derived(&TableRef::aliased("recent", "o"), &[derived("recent")]),
        Err(BuildError::AliasedDerivedRelation { name: "o" })
    );
}

#[test]
fn a_derived_relation_registered_under_the_name_it_is_addressed_by_is_accepted() {
    assert_eq!(
        reject_shadowed_derived(&TableRef::aliased("recent", "o"), &[derived("o")]),
        Ok(())
    );
    assert_eq!(
        reject_shadowed_derived(&TableRef::plain("recent"), &[derived("recent")]),
        Ok(())
    );
    assert_eq!(
        reject_shadowed_derived(&TableRef::aliased("recent", "o"), &[]),
        Ok(())
    );
}

#[test]
fn registering_one_relation_twice_reports_the_effective_name() {
    let mut relations = Vec::new();
    assert_eq!(
        register_relation(&mut relations, TableRef::plain("users")),
        Ok(())
    );
    assert_eq!(
        register_relation(&mut relations, TableRef::plain("users")),
        Err(BuildError::DuplicateRelation { name: "users" })
    );
}

#[test]
fn alias_distinguished_occurrences_of_one_table_are_accepted() {
    let mut relations = Vec::new();
    assert_eq!(
        register_relation(&mut relations, TableRef::plain("accounts")),
        Ok(())
    );
    assert_eq!(
        register_relation(&mut relations, TableRef::aliased("accounts", "a")),
        Ok(())
    );
    // A reference resolves through the alias, so reusing it collides even though the
    // underlying tables differ.
    assert_eq!(
        register_relation(&mut relations, TableRef::aliased("customers", "a")),
        Err(BuildError::DuplicateRelation { name: "a" })
    );
}

#[test]
fn build_error_display_unresolved_column() {
    let err = BuildError::UnresolvedColumn {
        table: "orders",
        column: "total",
    };
    let msg = err.to_string();
    assert!(msg.contains("`orders.total`"));
    assert!(msg.contains("addresses no relation"));
    assert!(msg.contains("`FROM`"));
}

/// Builds a statement projecting `projections` without introducing any relation.
fn relation_free_statement(projections: Vec<ExprKind>) -> SelectAst {
    let mut ast = empty_ast();
    ast.projections = projections;
    ast
}

#[test]
fn a_plain_column_projected_by_a_statement_without_relations_is_rejected() {
    // The statement renders no `FROM` clause, so the bare table name of the column it
    // projects names a relation nothing declares.
    let ast = relation_free_statement(vec![ExprKind::Column {
        table: "orders",
        name: "total",
    }]);

    assert_eq!(
        ast.validate(),
        Err(BuildError::UnresolvedColumn {
            table: "orders",
            column: "total",
        })
    );
}

#[test]
fn a_plain_column_read_by_a_filter_without_relations_is_rejected() {
    let mut ast = relation_free_statement(vec![ExprKind::Value(SqlVal::I64(1))]);
    ast.filter = Some(ExprKind::Binary {
        op: BinaryOp::Gt,
        lhs: Box::new(ExprKind::Column {
            table: "orders",
            name: "total",
        }),
        rhs: Box::new(ExprKind::Value(SqlVal::F64(0.0))),
    });

    assert_eq!(
        ast.validate(),
        Err(BuildError::UnresolvedColumn {
            table: "orders",
            column: "total",
        })
    );
}

#[test]
fn a_plain_column_projected_beside_the_relation_it_names_is_accepted() {
    // The statement introduces the relation the reference names, so the renderer
    // qualifies the column against it.
    let ast = super::orders_statement(vec![ExprKind::Column {
        table: "orders",
        name: "total",
    }]);

    assert_eq!(ast.validate(), Ok(()));
}

#[test]
fn a_nested_statement_reaches_a_plain_column_of_an_enclosing_relation() {
    // The nested statement introduces no relation of its own, so the bare reference
    // resolves by correlation through the relations in scope where it is written.
    let mut inner = empty_ast();
    inner.projections.push(ExprKind::Column {
        table: "orders",
        name: "total",
    });

    assert_eq!(
        crate::query::validate::reject_unresolved_qualifiers(
            &scalar_subquery(inner),
            &[TableRef::plain("orders")],
            &[],
        ),
        Ok(())
    );
}

#[test]
fn a_nested_statement_reading_a_plain_column_of_no_relation_is_rejected() {
    // Nothing encloses the nested statement with a relation and it introduces none
    // itself, so the bare reference names a relation the rendered statement never
    // declares.
    let mut inner = empty_ast();
    inner.projections.push(ExprKind::Column {
        table: "orders",
        name: "total",
    });

    assert_eq!(
        crate::query::validate::reject_unresolved_qualifiers(&scalar_subquery(inner), &[], &[]),
        Err(BuildError::UnresolvedColumn {
            table: "orders",
            column: "total",
        })
    );
}

#[test]
fn a_plain_column_naming_a_relation_the_statement_does_not_address_is_rejected() {
    // The statement reads `orders` while the reference names `recent`: no relation in
    // scope carries that name, so the rendered qualifier would address a table the
    // engine cannot find.
    let mut ast = relation_statement("orders", "id");
    ast.projections.push(ExprKind::Column {
        table: "recent",
        name: "total",
    });

    assert_eq!(
        ast.validate(),
        Err(BuildError::UnresolvedColumn {
            table: "recent",
            column: "total",
        })
    );
}

#[test]
fn a_plain_column_naming_the_alias_of_an_aliased_relation_is_accepted() {
    // The statement declares the relation as `"orders" AS "o"`, so a reference
    // naming `o` reaches it through the alias the renderer writes verbatim.
    let mut ast = empty_ast();
    ast.from = Some(TableRef::aliased("orders", "o"));
    ast.projections.push(ExprKind::Column {
        table: "o",
        name: "id",
    });

    assert_eq!(ast.validate(), Ok(()));
}

#[test]
fn a_plain_column_naming_the_table_behind_an_alias_is_accepted() {
    // The renderer substitutes the alias of an aliased occurrence for the table name
    // the reference carries, so the statement stays resolvable.
    let mut ast = empty_ast();
    ast.from = Some(TableRef::aliased("orders", "o"));
    ast.projections.push(ExprKind::Column {
        table: "orders",
        name: "id",
    });

    assert_eq!(ast.validate(), Ok(()));
}

#[test]
fn a_plain_column_naming_a_derived_relation_is_accepted() {
    // A derived table joins the `FROM` clause under the name it is exposed as, so a
    // reference naming it resolves like a reference to a stored relation.
    let mut ast = empty_ast();
    ast.from = Some(TableRef::plain("totals"));
    ast.projections.push(ExprKind::Column {
        table: "totals",
        name: "total",
    });
    ast.derived.push(derived("totals"));

    assert_eq!(ast.validate(), Ok(()));
}

#[test]
fn a_nested_plain_column_correlating_to_the_enclosing_statement_is_accepted() {
    // The nested statement introduces `orders` itself and reads `users` from the
    // statement enclosing it, which is exactly what correlation provides.
    let mut inner = relation_statement("orders", "id");
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

    let mut ast = relation_statement("users", "id");
    ast.filter = Some(exists_subquery(inner));

    assert_eq!(ast.validate(), Ok(()));
}

#[test]
fn a_nested_plain_column_naming_no_relation_in_scope_is_rejected() {
    // Neither the nested statement nor the one enclosing it introduces `ghosts`, so
    // the reference resolves to nothing at any level of the statement tree.
    let mut inner = relation_statement("orders", "id");
    inner.filter = Some(ExprKind::Column {
        table: "ghosts",
        name: "id",
    });

    let mut ast = relation_statement("users", "id");
    ast.filter = Some(exists_subquery(inner));

    assert_eq!(
        ast.validate(),
        Err(BuildError::UnresolvedColumn {
            table: "ghosts",
            column: "id",
        })
    );
}

#[test]
fn a_plain_column_projection_built_through_the_public_constructor_is_accepted() {
    // A reference assembled through `Expr::from_kind` names the relation the
    // statement introduces, so the membership rule accepts it exactly like the
    // schema-marker shape the fluent builders produce.
    let ast = super::orders_statement(vec![Expr::<f64>::from_kind(ExprKind::Column {
        table: "orders",
        name: "total",
    })
    .into_kind()]);

    assert_eq!(ast.validate(), Ok(()));
}
