//! The `ON CONFLICT` clause and the action a violated target triggers.
//!
//! [`Conflict`] binds the conflict target and the predicate deciding which rows
//! that target covers to the action taken when the target is violated, while
//! [`ConflictAction`] records whether the conflicting row is skipped or rewritten
//! through typed column assignments.
//!
//! The clause validates the expressions it renders before any SQL is produced and
//! appends itself to the statement in a single rendering pass, so the statement
//! carries no rendering closure of its own.

use crate::dialect::Dialect;
use crate::dml::AssignValue;
use crate::expr::{render_expr, ExprKind, RawSqlError, SqlVal};
use crate::query::{
    reject_unresolved_qualifiers, validate_expr, validate_row_clause, BuildError, RowClause,
};
use crate::schema::TableRef;

/// The action executed when an inserted row violates the conflict target.
#[derive(Debug, Clone, PartialEq)]
pub enum ConflictAction {
    /// `DO NOTHING`, silently skipping the conflicting row.
    DoNothing,
    /// `DO UPDATE SET ...`, applying typed assignments to the existing row.
    ///
    /// The branch carries at least one assignment: a clause recording none is
    /// rejected while the statement is built, so it can never be degraded into
    /// a `DO NOTHING` that drops the recorded predicate.
    DoUpdate {
        /// The column assignments applied to the conflicting row.
        assignments: Vec<(&'static str, AssignValue)>,
        /// The predicate restricting the rows the assignments apply to.
        predicate: Option<ExprKind>,
    },
}

/// An `ON CONFLICT` clause attached to an `INSERT` statement.
///
/// An empty [`target`](Conflict::target) conflicts on any unique constraint of
/// the table, which is the form used by a bare `DO NOTHING`.
#[derive(Debug, Clone, PartialEq)]
pub struct Conflict {
    /// The conflict target columns, empty when no target is given.
    pub target: Vec<&'static str>,
    /// The predicate the conflict target is restricted to, when one is given.
    ///
    /// The predicate addresses the rows a partial unique index covers, so it
    /// closes the target columns and precedes the action taken on them.
    pub target_predicate: Option<ExprKind>,
    /// The action executed on conflict.
    pub action: ConflictAction,
}

impl Conflict {
    /// Validates the expressions and assignments embedded in the clause.
    ///
    /// The target predicate, the update predicate, and an expression assignment
    /// of the `DO UPDATE` branch may all carry a nested statement, so the clause
    /// is audited before the statement renders and a semantic violation inside
    /// one of them is reported as [`BuildError`].
    ///
    /// `relations` names the relations the clause is rendered against — the table
    /// the row is inserted into — so a reference qualified through a name that
    /// table does not declare is reported as
    /// [`BuildError::UnresolvedQualifier`] as well. The proposed row of
    /// `EXCLUDED` carries no qualifier of its own, so reading it stays accepted.
    ///
    /// Both predicates and the assignments are per-row positions — they decide
    /// which conflicting row is rewritten and to which value — so an aggregate
    /// call inside one of them is rejected as
    /// [`BuildError::AggregateInRowClause`] before any SQL is rendered.
    ///
    /// A `DO UPDATE` branch records at least one assignment: a branch naming no
    /// column to rewrite could only render `DO UPDATE SET` with nothing to
    /// assign or degrade into a `DO NOTHING` that carries no predicate at all,
    /// so the emptiness is reported as [`BuildError::EmptyConflictUpdateSet`]
    /// instead of silently dropping what the caller recorded.
    pub(crate) fn validate(&self, relations: &[TableRef]) -> Result<(), BuildError> {
        if let Some(predicate) = &self.target_predicate {
            validate_qualifiers(predicate, relations, RowClause::ConflictTarget)?;
        }
        match &self.action {
            ConflictAction::DoNothing => Ok(()),
            ConflictAction::DoUpdate {
                assignments,
                predicate,
            } => {
                // A branch that names no column to rewrite can only render
                // `DO UPDATE SET` with nothing to assign, or be degraded into a
                // `DO NOTHING` that carries no predicate at all — silently
                // dropping what `update_where` recorded. The emptiness is
                // rejected here instead, before a single character of the
                // statement is rendered.
                if assignments.is_empty() {
                    return Err(BuildError::EmptyConflictUpdateSet);
                }
                for (_, value) in assignments {
                    value.validate()?;
                    value.validate_qualifiers(relations, &[])?;
                }
                if let Some(predicate) = predicate {
                    validate_qualifiers(predicate, relations, RowClause::ConflictUpdate)?;
                }
                Ok(())
            }
        }
    }

    /// Appends the clause to `sql`, binding every value to `params`.
    ///
    /// The predicates and the expression assignments of the clause may embed a
    /// raw expression whose fragments disagree with its parameters, so the
    /// walk reports the census mismatch as [`RawSqlError`] instead of
    /// dispatching SQL with a missing or a misplaced placeholder.
    pub(crate) fn render<D: Dialect>(
        &self,
        sql: &mut String,
        params: &mut Vec<SqlVal>,
    ) -> Result<(), RawSqlError> {
        sql.push_str(" ON CONFLICT");
        if !self.target.is_empty() {
            sql.push_str(" (");
            for (index, name) in self.target.iter().enumerate() {
                if index > 0 {
                    sql.push_str(", ");
                }
                D::push_quoted_identifier(sql, name);
            }
            sql.push(')');
        }

        // The target predicate selects the index the statement conflicts on, so
        // it closes the target before the action follows. Both predicates address
        // the columns of the inserted table unqualified, which every dialect
        // accepts, while the proposed row names its own `EXCLUDED` relation.
        if let Some(predicate) = &self.target_predicate {
            sql.push_str(" WHERE ");
            render_expr::<D>(sql, params, predicate, &[])?;
        }

        match &self.action {
            ConflictAction::DoNothing => sql.push_str(" DO NOTHING"),
            ConflictAction::DoUpdate {
                assignments,
                predicate,
            } => {
                // A branch records at least one assignment, because
                // `Conflict::validate` rejects an empty one before the statement
                // reaches the renderer.
                sql.push_str(" DO UPDATE SET ");
                for (index, (name, value)) in assignments.iter().enumerate() {
                    if index > 0 {
                        sql.push_str(", ");
                    }
                    D::push_quoted_identifier(sql, name);
                    sql.push_str(" = ");
                    // The clause addresses the inserted row and its proposed
                    // `EXCLUDED` row, both of which are referenced explicitly, so
                    // a column of the target table stays unqualified.
                    value.render::<D>(sql, params, &[])?;
                }
                if let Some(predicate) = predicate {
                    sql.push_str(" WHERE ");
                    render_expr::<D>(sql, params, predicate, &[])?;
                }
            }
        }

        Ok(())
    }
}

/// Validates a predicate of the clause and the relation qualifiers it names.
///
/// The clause addresses the inserted table as well as the proposed row its
/// `EXCLUDED` relation exposes, so `relations` — the relations the prepared
/// statement renders — is the namespace a qualifier resolves against, exactly
/// like the one the renderer hands to every expression of the clause.
///
/// The predicate is evaluated once per candidate row — the target predicate
/// selects the index the statement conflicts on, and the update predicate decides
/// which conflicting rows are rewritten — so `clause` names the position the
/// aggregate placement rule reports when the predicate carries an aggregate call.
///
/// The statements the predicate embeds are audited structurally as well, exactly
/// like those of a filter or a `RETURNING` projection: a predicate is an
/// expression of the statement, so a semantic violation inside a nested statement
/// it carries is reported before any SQL is rendered.
fn validate_qualifiers(
    expr: &ExprKind,
    relations: &[TableRef],
    clause: RowClause,
) -> Result<(), BuildError> {
    validate_row_clause(expr, clause)?;
    validate_expr(expr)?;
    reject_unresolved_qualifiers(expr, relations, &[])
}
