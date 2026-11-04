//! Clauses of the mutation statements inside the consolidated pass.
//!
//! The DML builders render clauses of their own — the `RETURNING` projections, the
//! `SET` assignments, and the filters — and those are expressions of the statement
//! exactly like a projection is, so the same pass audits the statements they embed
//! and resolves the qualifiers they name. The tests below pin the rejections down on
//! the public AST surface, where the tier-1 typestate cannot reach.

use super::super::{Order, OrderId, OrderTotal, RecentId, RecentTotal};
use super::{
    count_star, exists_subquery, relation_statement, scalar_subquery, sum_of, UpsertDialect,
};
use crate::expr::{BinaryOp, Expr, ExprKind, JoinAst, JoinKind, SelectAst, SqlVal, WindowExprKind};
use crate::query::*;
use crate::schema::TableRef;

#[test]
fn an_insert_value_addresses_no_relation_at_all() {
    use crate::dml::{ColumnAssign, InsertQuery};

    // The `VALUES` list of an `INSERT INTO "orders"` renders the values of one row and
    // declares no relation, so even an expression reading `recent.total` explicitly
    // names a table the statement cannot resolve.
    let built = InsertQuery::<UpsertDialect, Order>::new()
        .values(OrderTotal::set_expr(Expr::<f64>::from_kind(
            ExprKind::QualifiedColumn {
                table: "recent",
                column: "total",
            },
        )))
        .build();

    assert_eq!(
        built,
        Err(BuildError::ColumnWithoutRelation {
            table: "recent",
            column: "total",
        })
    );
    }

#[test]
fn an_insert_value_referencing_the_excluded_relation_is_rejected() {
    use crate::dml::{ColumnAssign, InsertQuery};

    // `EXCLUDED` names the row an upsert proposed, so it exists only inside the
    // `ON CONFLICT DO UPDATE` branch of the statement. The `VALUES` list of an
    // `INSERT` admits no conflict, so the reference renders a relation no engine
    // resolves there. The type system keeps the typed reference out of the
    // builder, while a tree assembled past it through `from_kind` reports the
    // empty scope and is caught here.
    let built = InsertQuery::<UpsertDialect, Order>::new()
        .values(OrderId::set_expr(Expr::<i64>::from_kind(
            ExprKind::Excluded { column: "id" },
        )))
        .build();

    assert_eq!(
        built,
        Err(BuildError::ExcludedWithoutConflict { column: "id" })
    );
}

#[test]
fn an_insert_value_nesting_the_excluded_relation_is_rejected() {
    use crate::dml::{ColumnAssign, InsertQuery};

    // The rule descends into a computed assignment as well: the reference stays
    // unresolvable wherever the `VALUES` list reaches it.
    let built = InsertQuery::<UpsertDialect, Order>::new()
        .values(OrderId::set_expr(Expr::<i64>::from_kind(
            ExprKind::Binary {
                op: BinaryOp::Add,
                lhs: Box::new(ExprKind::Excluded { column: "id" }),
                rhs: Box::new(ExprKind::Value(SqlVal::I64(1))),
            },
        )))
        .build();

    assert_eq!(
        built,
        Err(BuildError::ExcludedWithoutConflict { column: "id" })
    );
}

#[test]
fn an_insert_value_embedding_a_correlated_subquery_is_rejected() {
    use crate::dml::{ColumnAssign, InsertQuery};

    // The `VALUES` list declares no relation, so a statement embedded in one of its
    // values reads only the relations it declares itself. The nested statement
    // declares `recent` while it filters on `orders`, which names the table the
    // statement writes — a relation the row cannot read, and a qualifier no engine
    // finds there.
    let mut inner = relation_statement("recent", "id");
    inner.filter = Some(ExprKind::Binary {
        op: BinaryOp::Eq,
        lhs: Box::new(ExprKind::Column {
            table: "orders",
            name: "id",
        }),
        rhs: Box::new(ExprKind::Column {
            table: "recent",
            name: "id",
        }),
    });

    let built = InsertQuery::<UpsertDialect, Order>::new()
        .values(OrderId::set_expr(Expr::<i64>::from_kind(scalar_subquery(inner))))
        .build();

    assert_eq!(
        built,
        Err(BuildError::UnresolvedColumn {
            table: "orders",
            column: "id",
        })
    );
}

#[test]
fn an_insert_value_embedding_a_subquery_qualified_by_a_foreign_relation_is_rejected() {
    use crate::dml::{ColumnAssign, InsertQuery};

    // A reference built through a table alias renders its qualifier verbatim, so the
    // nested statement has to declare it: the `VALUES` row brings no relation the
    // alias could belong to.
    let mut inner = relation_statement("recent", "id");
    inner.projections = vec![ExprKind::QualifiedColumn {
        table: "o",
        column: "id",
    }];

    let built = InsertQuery::<UpsertDialect, Order>::new()
        .values(OrderId::set_expr(Expr::<i64>::from_kind(scalar_subquery(inner))))
        .build();

    assert_eq!(built, Err(BuildError::UnresolvedQualifier { name: "o" }));
}

#[test]
fn an_insert_value_embedding_a_subquery_whose_nested_statement_correlates_is_rejected() {
    use crate::dml::{ColumnAssign, InsertQuery};

    // The rule descends through the statements a value embeds: an `EXISTS` predicate
    // written inside the scalar subquery reaches no further namespace either.
    let mut deepest = relation_statement("recent", "id");
    deepest.filter = Some(ExprKind::Binary {
        op: BinaryOp::Eq,
        lhs: Box::new(ExprKind::Column {
            table: "accounts",
            name: "id",
        }),
        rhs: Box::new(ExprKind::Value(SqlVal::I64(1))),
    });

    let mut middle = relation_statement("recent", "id");
    middle.filter = Some(exists_subquery(deepest));

    let built = InsertQuery::<UpsertDialect, Order>::new()
        .values(OrderId::set_expr(Expr::<i64>::from_kind(scalar_subquery(
            middle,
        ))))
        .build();

    assert_eq!(
        built,
        Err(BuildError::UnresolvedColumn {
            table: "accounts",
            column: "id",
        })
    );
}

#[test]
fn an_insert_value_embedding_a_self_contained_subquery_is_accepted() {
    use crate::dml::{ColumnAssign, InsertQuery};

    // The nested statement declares the relation its aggregate reads, so the row
    // computes one value per inserted row without reaching out of its own scope.
    let mut inner = relation_statement("recent", "total");
    inner.projections = vec![sum_of("recent", "total")];

    let built = InsertQuery::<UpsertDialect, Order>::new()
        .values(OrderId::set_expr(Expr::<i64>::from_kind(scalar_subquery(inner))))
        .build()
        .expect("the nested statement aggregates its own relation");

    assert_eq!(
        built.sql,
        "INSERT INTO \"orders\" (\"id\") VALUES ((SELECT SUM(\"total\") FROM \"recent\"))"
    );
    assert!(built.params.is_empty());
}

#[test]
fn build_error_display_excluded_without_conflict() {
    let msg = BuildError::ExcludedWithoutConflict { column: "id" }.to_string();
    assert!(msg.contains("EXCLUDED.id"));
    assert!(msg.contains("ON CONFLICT DO UPDATE"));
    assert!(msg.contains("constants and parameters"));
}

#[test]
fn a_conflict_target_predicate_qualified_by_a_foreign_relation_is_rejected() {
    use crate::dml::{ColumnAssign, InsertQuery};

    let built = InsertQuery::<UpsertDialect, Order>::new()
        .values((OrderId::set(1i64), OrderTotal::set(2.5f64)))
        .on_conflict((OrderId,))
        .target_where(Expr::column::<RecentId>().without_scope().gt(0i64))
        .do_nothing()
        .build();

    assert_eq!(
        built,
        Err(BuildError::UnresolvedQualifier { name: "recent" })
    );
}

#[test]
fn a_conflict_update_predicate_qualified_by_a_foreign_relation_is_rejected() {
    use crate::dml::{ColumnAssign, InsertQuery};

    let built = InsertQuery::<UpsertDialect, Order>::new()
        .values((OrderId::set(1i64), OrderTotal::set(2.5f64)))
        .on_conflict((OrderId,))
        .update_where(Expr::column::<RecentTotal>().without_scope().gt(0.0f64))
        .do_update(OrderTotal::set(0.0f64))
        .build();

    assert_eq!(
        built,
        Err(BuildError::UnresolvedQualifier { name: "recent" })
    );
}

#[test]
fn a_conflict_assignment_reading_a_foreign_relation_is_rejected() {
    use crate::dml::{ColumnAssign, InsertQuery};

    let built = InsertQuery::<UpsertDialect, Order>::new()
        .values((OrderId::set(1i64), OrderTotal::set(2.5f64)))
        .on_conflict((OrderId,))
        .do_update(OrderTotal::set_expr(
            Expr::column::<RecentTotal>().without_scope(),
        ))
        .build();

    assert_eq!(
        built,
        Err(BuildError::UnresolvedQualifier { name: "recent" })
    );
}

#[test]
fn an_upsert_addressing_the_target_relation_and_its_proposed_row_is_accepted() {
    use crate::dml::{excluded, ColumnAssign, InsertQuery};

    // Both predicates qualify their references through the inserted table, and the
    // assignment copies the proposed row, which names no qualifier of its own.
    let built = InsertQuery::<UpsertDialect, Order>::new()
        .values((OrderId::set(1i64), OrderTotal::set(2.5f64)))
        .on_conflict((OrderId,))
        .target_where(Expr::column::<OrderId>().without_scope().gt(0i64))
        .update_where(Expr::column::<OrderTotal>().without_scope().gt(0.0f64))
        .do_update(OrderTotal::set_expr(excluded(OrderTotal)))
        .build()
        .expect("the clause addresses the inserted table and its proposed row");

    assert_eq!(
        built.sql,
        "INSERT INTO \"orders\" (\"id\", \"total\") VALUES ($1, $2) \
         ON CONFLICT (\"id\") WHERE (\"orders\".\"id\" > $3) \
         DO UPDATE SET \"total\" = EXCLUDED.\"total\" WHERE (\"orders\".\"total\" > $4)"
    );
    assert_eq!(
        built.params,
        vec![
            SqlVal::I64(1),
            SqlVal::F64(2.5),
            SqlVal::I64(0),
            SqlVal::F64(0.0),
        ]
    );
}

/// A statement addressing `orders` twice without an alias.
///
/// No engine accepts the `FROM` clause it renders, so every clause embedding it
/// describes an invalid statement — which is what the DML trunk tests need.
fn duplicated_relation_statement() -> SelectAst {
    let mut ast = relation_statement("orders", "id");
    ast.joins.push(JoinAst {
        kind: JoinKind::Inner,
        table: TableRef::plain("orders"),
        lateral: None,
        on: None,
    });
    ast
}

/// A `RETURNING` clause whose single projection is chosen by the test.
///
/// The clause declares a projected column of `Order`, so the DML builders accept it
/// as a selection of the mutated table, while the projection it records is supplied
/// by the test: the fluent selection types cannot describe a projection embedding an
/// invalid statement, which is exactly the shape the pre-flight pass has to catch.
pub(super) struct TrunkReturning(pub(super) ExprKind);

impl Selection for TrunkReturning {
    type Row = (i64,);

    fn push_projections(&self, out: &mut Vec<ExprKind>) {
        out.push(self.0.clone());
    }
}

impl TableSelection<Order> for TrunkReturning {
    fn push_column_names(&self, _out: &mut Vec<&'static str>) {}
}

#[test]
fn an_insert_returning_projection_embedding_an_invalid_statement_is_rejected() {
    use crate::dml::{ColumnAssign, InsertQuery};

    // The `RETURNING` clause belongs to the same statement, so a nested statement it
    // embeds is audited exactly like a projection of a `SELECT`.
    let built = InsertQuery::<UpsertDialect, Order>::new()
        .values((OrderId::set(1i64), OrderTotal::set(2.5f64)))
        .returning(TrunkReturning(scalar_subquery(
            duplicated_relation_statement(),
        )))
        .build();

    assert_eq!(built, Err(BuildError::DuplicateRelation { name: "orders" }));
}

#[test]
fn an_update_returning_projection_embedding_an_invalid_statement_is_rejected() {
    use crate::dml::{ColumnAssign, UpdateQuery};

    let built = UpdateQuery::<UpsertDialect, Order>::new()
        .set(OrderTotal::set(2.5f64))
        .where_(Expr::column::<OrderId>().eq(1i64))
        .returning(TrunkReturning(scalar_subquery(
            duplicated_relation_statement(),
        )))
        .build();

    assert_eq!(built, Err(BuildError::DuplicateRelation { name: "orders" }));
}

#[test]
fn a_delete_returning_projection_embedding_an_invalid_statement_is_rejected() {
    use crate::dml::DeleteQuery;

    let built = DeleteQuery::<UpsertDialect, Order>::new()
        .where_(Expr::column::<OrderId>().eq(1i64))
        .returning(TrunkReturning(scalar_subquery(
            duplicated_relation_statement(),
        )))
        .build();

    assert_eq!(built, Err(BuildError::DuplicateRelation { name: "orders" }));
}

#[test]
fn an_update_assignment_embedding_an_invalid_statement_is_rejected() {
    use crate::dml::{ColumnAssign, UpdateQuery};

    // The assigned value is an expression of the statement, so it is audited by the
    // same pass that audits a filter or a projection.
    let built = UpdateQuery::<UpsertDialect, Order>::new()
        .set(OrderTotal::set_expr(
            Expr::<f64>::from_kind(scalar_subquery(duplicated_relation_statement()))
                .without_scope(),
        ))
        .allow_all()
        .build();

    assert_eq!(built, Err(BuildError::DuplicateRelation { name: "orders" }));
}

#[test]
fn an_insert_row_source_collapsing_to_no_assignment_is_rejected() {
    use crate::dml::insert::InsertSource;
    use crate::dml::{AssignColumn, AssignValue, AssignmentScope, AssignmentSet, InsertQuery};

    /// A row source collapsing to no assignment at all.
    ///
    /// The fluent builders keep such a row out at the call site, but the traits
    /// describing a row are public, so the emptiness has to be reported while the
    /// statement is built instead of by the engine.
    struct EmptyRow;

    impl AssignmentSet<Order> for EmptyRow {
        fn extend_assignments<'a>(&'a self, _out: &mut Vec<AssignColumn<'a>>) {}

        fn into_assignments(self, _out: &mut Vec<(&'static str, AssignValue)>) {}
    }

    impl AssignmentScope for EmptyRow {
        type Scope = ();
        type Dialect = ();
    }

    impl InsertSource<Order> for EmptyRow {}

    let built = InsertQuery::<UpsertDialect, Order>::new()
        .values(EmptyRow)
        .build();

    assert_eq!(built, Err(BuildError::EmptyInsertRow { row: 0 }));
}

#[test]
fn build_error_display_empty_insert_row() {
    let msg = BuildError::EmptyInsertRow { row: 2 }.to_string();
    assert!(msg.contains("row 2"));
    assert!(msg.contains("VALUES ()"));
    assert!(msg.contains(".from_select(...)"));
}

#[test]
fn an_insert_returning_projection_qualified_by_a_foreign_relation_is_rejected() {
    use crate::dml::{ColumnAssign, InsertQuery};

    // The clause belongs to the statement that inserts into `orders`, so a reference
    // qualified through another relation names nothing the rendered statement
    // declares.
    let built = InsertQuery::<UpsertDialect, Order>::new()
        .values(OrderId::set(1i64))
        .returning(TrunkReturning(ExprKind::QualifiedColumn {
            table: "recent",
            column: "id",
        }))
        .build();

    assert_eq!(
        built,
        Err(BuildError::UnresolvedQualifier { name: "recent" })
    );
}

#[test]
fn an_update_returning_projection_reading_a_foreign_column_is_rejected() {
    use crate::dml::{ColumnAssign, UpdateQuery};

    // A plain column reference names its relation through the node itself, so the
    // clause is rejected through the membership rule rather than the qualifier rule.
    let built = UpdateQuery::<UpsertDialect, Order>::new()
        .set(OrderTotal::set(2.5f64))
        .where_(Expr::column::<OrderId>().eq(1i64))
        .returning(TrunkReturning(ExprKind::Column {
            table: "recent",
            name: "total",
        }))
        .build();

    assert_eq!(
        built,
        Err(BuildError::UnresolvedColumn {
            table: "recent",
            column: "total",
        })
    );
}

#[test]
fn a_delete_returning_projection_qualified_by_a_foreign_relation_is_rejected() {
    use crate::dml::DeleteQuery;

    // The deletion renders its own target only, so an alias-qualified projection
    // names a relation the statement never declares.
    let built = DeleteQuery::<UpsertDialect, Order>::new()
        .where_(Expr::column::<OrderId>().eq(1i64))
        .returning(TrunkReturning(ExprKind::QualifiedColumn {
            table: "recent",
            column: "total",
        }))
        .build();

    assert_eq!(
        built,
        Err(BuildError::UnresolvedQualifier { name: "recent" })
    );
}

#[test]
fn a_returning_projection_repeating_the_mutated_target_is_accepted() {
    use crate::dml::{ColumnAssign, UpdateQuery};

    // The projection names the mutated relation under the name the statement renders
    // it by, so the clause renders unqualified beside the single relation.
    let built = UpdateQuery::<UpsertDialect, Order>::new()
        .set(OrderTotal::set(2.5f64))
        .where_(Expr::column::<OrderId>().eq(1i64))
        .returning(TrunkReturning(ExprKind::Column {
            table: "orders",
            name: "total",
        }))
        .build()
        .expect("the returning projection addresses the mutated relation");

    assert_eq!(
        built.sql,
        "UPDATE \"orders\" SET \"total\" = $1 WHERE (\"id\" = $2) RETURNING \"total\""
    );
    assert_eq!(built.params, vec![SqlVal::F64(2.5), SqlVal::I64(1)]);
}

/// The rejection a `RETURNING` projection carrying an aggregate is reported as.
fn aggregate_in_returning() -> BuildError {
    BuildError::AggregateInRowClause {
        clause: RowClause::Returning,
        aggregate: "COUNT",
    }
}

/// The rejection a `RETURNING` projection carrying a window call is reported as.
fn window_in_returning() -> BuildError {
    BuildError::WindowInRowClause {
        clause: RowClause::Returning,
    }
}

/// A `ROW_NUMBER()` window call, which a `RETURNING` clause cannot evaluate.
///
/// The call reads the rows a statement produced, while a mutation hands its rows
/// back one at a time, so the clause offers it no row set to read.
fn row_number_window() -> ExprKind {
    ExprKind::Window {
        kind: Box::new(WindowExprKind::RowNumber),
        partition_by: Vec::new(),
        order_by: Vec::new(),
        frame: None,
    }
}

#[test]
fn an_insert_returning_projection_carrying_an_aggregate_is_rejected() {
    use crate::dml::{ColumnAssign, InsertQuery};

    // The clause reports each inserted row on its own, so an aggregate written
    // into it would collapse the rows the statement is handing back.
    let built = InsertQuery::<UpsertDialect, Order>::new()
        .values((OrderId::set(1i64), OrderTotal::set(2.5f64)))
        .returning(TrunkReturning(count_star()))
        .build();

    assert_eq!(built, Err(aggregate_in_returning()));
}

#[test]
fn an_update_returning_projection_carrying_an_aggregate_is_rejected() {
    use crate::dml::{ColumnAssign, UpdateQuery};

    let built = UpdateQuery::<UpsertDialect, Order>::new()
        .set(OrderTotal::set(2.5f64))
        .where_(Expr::column::<OrderId>().eq(1i64))
        .returning(TrunkReturning(count_star()))
        .build();

    assert_eq!(built, Err(aggregate_in_returning()));
}

#[test]
fn a_delete_returning_projection_carrying_an_aggregate_is_rejected() {
    use crate::dml::DeleteQuery;

    let built = DeleteQuery::<UpsertDialect, Order>::new()
        .where_(Expr::column::<OrderId>().eq(1i64))
        .returning(TrunkReturning(count_star()))
        .build();

    assert_eq!(built, Err(aggregate_in_returning()));
}

#[test]
fn an_insert_returning_projection_carrying_a_window_call_is_rejected() {
    use crate::dml::{ColumnAssign, InsertQuery};

    // The window rule is a branch of its own: a window call is not an aggregate,
    // so only that branch reports it, and the aggregate branch stays silent.
    let built = InsertQuery::<UpsertDialect, Order>::new()
        .values((OrderId::set(1i64), OrderTotal::set(2.5f64)))
        .returning(TrunkReturning(row_number_window()))
        .build();

    assert_eq!(built, Err(window_in_returning()));
}

#[test]
fn an_update_returning_projection_carrying_a_window_call_is_rejected() {
    use crate::dml::{ColumnAssign, UpdateQuery};

    let built = UpdateQuery::<UpsertDialect, Order>::new()
        .set(OrderTotal::set(2.5f64))
        .where_(Expr::column::<OrderId>().eq(1i64))
        .returning(TrunkReturning(row_number_window()))
        .build();

    assert_eq!(built, Err(window_in_returning()));
}

#[test]
fn a_delete_returning_projection_carrying_a_window_call_is_rejected() {
    use crate::dml::DeleteQuery;

    let built = DeleteQuery::<UpsertDialect, Order>::new()
        .where_(Expr::column::<OrderId>().eq(1i64))
        .returning(TrunkReturning(row_number_window()))
        .build();

    assert_eq!(built, Err(window_in_returning()));
}

#[test]
fn a_returning_projection_reports_its_aggregate_before_an_invalid_embedded_statement() {
    use crate::dml::{ColumnAssign, InsertQuery};

    // The projection is both an aggregate and a statement the pre-flight pass
    // refuses, so the two rules meet on one clause: the row-clause rule runs
    // first, which is what `validate_row_clause` preceding `validate_expr`
    // records.
    let projection = ExprKind::Binary {
        op: BinaryOp::Gt,
        lhs: Box::new(count_star()),
        rhs: Box::new(scalar_subquery(duplicated_relation_statement())),
    };

    let built = InsertQuery::<UpsertDialect, Order>::new()
        .values((OrderId::set(1i64), OrderTotal::set(2.5f64)))
        .returning(TrunkReturning(projection))
        .build();

    assert_eq!(built, Err(aggregate_in_returning()));
}

#[test]
fn a_returning_projection_reading_an_aggregating_subquery_is_accepted() {
    use crate::dml::{ColumnAssign, UpdateQuery};

    // The aggregate belongs to the nested statement, which collapses its own rows
    // into the one value the projection reads: the rule stops at the statement
    // boundary.
    let mut inner = relation_statement("orders", "total");
    inner.projections = vec![sum_of("orders", "total")];

    let built = UpdateQuery::<UpsertDialect, Order>::new()
        .set(OrderTotal::set(2.5f64))
        .where_(Expr::column::<OrderId>().eq(1i64))
        .returning(TrunkReturning(scalar_subquery(inner)))
        .build()
        .expect("a scalar subquery aggregates its own scope");

    assert_eq!(
        built.sql,
        "UPDATE \"orders\" SET \"total\" = $1 WHERE (\"id\" = $2) \
         RETURNING (SELECT SUM(\"total\") FROM \"orders\")"
    );
    assert_eq!(built.params, vec![SqlVal::F64(2.5), SqlVal::I64(1)]);
}

#[test]
fn a_returning_projection_reading_a_window_subquery_is_accepted() {
    use crate::dml::DeleteQuery;

    // The window call belongs to the nested statement, which reads its own row
    // set: the `RETURNING` clause offers the projection one modified row, while
    // the call reads the rows the subquery produced.
    let mut inner = relation_statement("orders", "id");
    inner.projections = vec![row_number_window()];

    let built = DeleteQuery::<UpsertDialect, Order>::new()
        .where_(Expr::column::<OrderId>().eq(1i64))
        .returning(TrunkReturning(scalar_subquery(inner)))
        .build()
        .expect("a window subquery reads its own row set");

    assert_eq!(
        built.sql,
        "DELETE FROM \"orders\" WHERE (\"id\" = $1) \
         RETURNING (SELECT ROW_NUMBER() OVER () FROM \"orders\")"
    );
    assert_eq!(built.params, vec![SqlVal::I64(1)]);
}

#[test]
fn build_error_display_window_in_row_clause() {
    let msg = window_in_returning().to_string();
    assert!(msg.contains("window function"));
    assert!(msg.contains("the `RETURNING` clause"));
    assert!(msg.contains("without a row set to read"));
}