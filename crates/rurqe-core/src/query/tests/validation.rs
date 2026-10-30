//! Pre-flight validation of the assembled statement, split by the area each rule
//! protects.
//!
//! The pass is stated once and reached by one section walk, so its tests are
//! grouped by what they exercise instead of growing into a single file: the
//! projection list, the `DISTINCT` and compound ordering rules, the raw SQL
//! census, the aggregation markers, the qualifier and relation namespaces, the
//! embedded statements, the DML trunk, and the row-level lock clause each own a
//! submodule.
//!
//! The shared fixtures live here — the minimal statement builders, the aggregate
//! helpers, and the upsert dialect — so every submodule reaches them through
//! `super::` instead of re-declaring them, and no single file grows into a
//! monolith.

mod aggregate_projection;
mod comparison;
mod compound_order;
mod cte;
mod distinct;
mod dml;
mod grouped;
mod locks;
mod placement;
mod projection;
mod qualifiers;
mod raw_sql;
mod relations;
mod scoping;
mod window;

use crate::expr::{
    AggregateFunc, ExprKind, RawSqlError, SelectAst, SetOpAst, SetOpKind, Subquery, SubqueryOp,
};
use crate::query::*;
use crate::schema::TableRef;

/// Builds a statement over `table` projecting its `column`.
fn relation_statement(table: &'static str, column: &'static str) -> SelectAst {
    let mut ast = empty_ast();
    ast.from = Some(TableRef::plain(table));
    ast.projections.push(ExprKind::Column {
        table,
        name: column,
    });
    ast
}

/// Builds a statement projecting a reference qualified by `qualifier` alone.
fn qualified_body(qualifier: &'static str) -> SelectAst {
    let mut ast = empty_ast();
    ast.projections.push(ExprKind::QualifiedColumn {
        table: qualifier,
        column: "id",
    });
    ast
}

/// Builds a statement projecting `projections` from the `orders` table.
fn orders_statement(projections: Vec<ExprKind>) -> SelectAst {
    let mut ast = empty_ast();
    ast.from = Some(TableRef::plain("orders"));
    ast.projections = projections;
    ast
}

/// The `orders.user_id` reference the grouping tests group by.
fn orders_user_id() -> ExprKind {
    ExprKind::Column {
        table: "orders",
        name: "user_id",
    }
}

/// A `COUNT(*)` aggregate expression.
fn count_star() -> ExprKind {
    ExprKind::Aggregate {
        func: AggregateFunc::Count,
        distinct: false,
        arg: None,
        filter: None,
    }
}

/// Builds a `SUM(table.name)` aggregate over one column.
fn sum_of(table: &'static str, name: &'static str) -> ExprKind {
    ExprKind::Aggregate {
        func: AggregateFunc::Sum,
        distinct: false,
        arg: Some(Box::new(ExprKind::Column { table, name })),
        filter: None,
    }
}

/// Builds a scalar subquery around `inner`.
fn scalar_subquery(inner: SelectAst) -> ExprKind {
    ExprKind::Subquery {
        op: SubqueryOp::Scalar,
        operand: None,
        query: Subquery::new(inner),
        negated: false,
    }
}

/// Builds an `EXISTS (...)` predicate around `inner`.
fn exists_subquery(inner: SelectAst) -> ExprKind {
    ExprKind::Subquery {
        op: SubqueryOp::Exists,
        operand: None,
        query: Subquery::new(inner),
        negated: false,
    }
}

/// A raw expression whose fragment/parameter census does not balance.
fn unbalanced_raw() -> ExprKind {
    ExprKind::Raw {
        fragments: vec!["1".to_owned(), " + 1".to_owned()],
        params: Vec::new(),
    }
}

/// The error an unbalanced raw expression is reported through.
fn unbalanced_raw_error() -> BuildError {
    BuildError::RawSql(RawSqlError::PlaceholderCount {
        expected: 1,
        provided: 0,
    })
}

/// A compound statement projecting `name` of `users` as `label` beside `total` of
/// `orders`.
///
/// Both operands project one column, while only the left one exposes its output
/// under a name — which is the output column a compound ordering may address. The
/// helper leaves the ordering to the test, so each regression below writes exactly
/// the criterion it exercises.
fn labelled_compound() -> SelectAst {
    let mut left = empty_ast();
    left.from = Some(TableRef::plain("users"));
    left.projections.push(ExprKind::Aliased {
        expr: Box::new(ExprKind::QualifiedColumn {
            table: "users",
            column: "name",
        }),
        alias: "label",
    });

    let mut right = empty_ast();
    right.from = Some(TableRef::plain("orders"));
    right.projections.push(ExprKind::Column {
        table: "orders",
        name: "total",
    });

    left.set_operations.push(SetOpAst {
        kind: SetOpKind::Union,
        query: right,
    });
    left
}

/// Builds a statement AST without any clause.
///
/// A statement recording no projection renders no select list, so every helper
/// above adds the clause the test under construction needs, and the pre-flight
/// pass rejects a statement that reaches it with an empty projection list.
fn empty_ast() -> SelectAst {
    SelectAst {
        ctes: Vec::new(),
        distinct: false,
        distinct_on: Vec::new(),
        projections: Vec::new(),
        from: None,
        joins: Vec::new(),
        derived: Vec::new(),
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
        lock: None,
    }
}

/// A dialect exposing the upsert clause the DML validation tests exercise.
///
/// The `ON CONFLICT` builder is only reachable on a dialect implementing
/// [`SupportsOnConflict`](crate::dialect::SupportsOnConflict), so the DML
/// regressions are checked against this dialect instead of the mock one the
/// query tests share.
struct UpsertDialect;

impl crate::dialect::Dialect for UpsertDialect {
    fn push_quoted_identifier(out: &mut String, ident: &str) {
        out.push('"');
        out.push_str(ident);
        out.push('"');
    }

    fn push_placeholder(out: &mut String, index: usize) {
        use core::fmt::Write;
        let _ = write!(out, "${index}");
    }
}

impl crate::dialect::SupportsOnConflict for UpsertDialect {}

// The DML trunk checks the clauses of the mutation statements, `RETURNING`
// projections included, so the dialect declares that capability as well: the
// clause stays reachable and the rejection comes from the pre-flight pass
// instead of from a missing trait bound.
impl crate::dialect::SupportsReturning for UpsertDialect {}
