//! Emptiness of the projection list.
//!
//! The projection list is what a statement renders between the `SELECT` keyword
//! and its `FROM` clause, so an empty one renders a statement no engine accepts.
//! The builder keeps the shape out of reach through the `ProjectionPresent`
//! typestate, while the list itself is recorded by `Selection::push_projections`
//! — a method an implementation outside the crate states itself — and by whatever
//! a hand-assembled statement carries, so the pre-flight pass rejects it on the
//! statement data.

use super::super::{MockDialect, Order};
use super::empty_ast;
use crate::expr::ExprKind;
use crate::query::*;
use crate::schema::TableRef;

#[test]
fn a_statement_recording_no_projection_is_rejected_before_rendering() {
    let mut ast = empty_ast();
    ast.from = Some(TableRef::plain("orders"));

    assert_eq!(ast.validate(), Err(BuildError::EmptyProjection));

    // One projected column is what makes the statement renderable again.
    ast.projections.push(ExprKind::Column {
        table: "orders",
        name: "id",
    });

    assert_eq!(ast.validate(), Ok(()));
}

/// A selection recording no projection at all.
///
/// The trait requires every implementation to state the items its rows are made of,
/// so the empty list is written out here on purpose: `select` moves the query into
/// `HasProjection` on the strength of the recorded list, which leaves the pre-flight
/// pass as the place that notices the list is empty.
struct MuteSelection;

impl Selection for MuteSelection {
    type Row = ();

    fn push_projections(&self, _out: &mut Vec<ExprKind>) {}
}

impl<Tables, D> crate::query::SelectionFor<Tables, (), D> for MuteSelection {
    type Row = ();
}

#[test]
fn a_selection_recording_no_projection_is_rejected_before_rendering() {
    let query = SelectQuery::<MockDialect>::new()
        .from::<Order>()
        .select::<MuteSelection, ()>(MuteSelection);

    assert_eq!(query.build(), Err(BuildError::EmptyProjection));
}

#[test]
fn build_error_display_empty_projection() {
    let msg = BuildError::EmptyProjection.to_string();
    assert!(msg.contains("no projected column"));
    assert!(msg.contains("SELECT  FROM"));
    assert!(msg.contains(".select(...)"));
}
