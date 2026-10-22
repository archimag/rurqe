//! `ON CONFLICT` (upsert) clause model and its single-pass renderer.
//!
//! [`Conflict`] records an optional conflict target together with the action
//! taken when that target is violated. The clause is attached to an
//! [`InsertQuery`] through [`InsertQuery::on_conflict`] or
//! [`InsertQuery::on_conflict_do_nothing`], both of which require a dialect
//! implementing [`SupportsOnConflict`], so a dialect without upsert support
//! simply does not compile.
//!
//! ```text
//! INSERT INTO "users" ("id", "name") VALUES ($1, $2) ON CONFLICT ("id") DO UPDATE SET "name" = $3
//! ```

use crate::dialect::{Dialect, SupportsOnConflict};
use crate::expr::SqlVal;
use crate::schema::Table;

use super::{AssignValue, AssignmentSet, InsertQuery};

/// The action executed when an inserted row violates the conflict target.
#[derive(Debug, Clone, PartialEq)]
pub enum ConflictAction {
    /// `DO NOTHING`, silently skipping the conflicting row.
    DoNothing,
    /// `DO UPDATE SET ...`, applying typed assignments to the existing row.
    DoUpdate(Vec<(&'static str, AssignValue)>),
}

/// An `ON CONFLICT` clause attached to an `INSERT` statement.
///
/// An empty [`target`](Conflict::target) conflicts on any unique constraint of
/// the table, which is the form used by a bare `DO NOTHING`.
#[derive(Debug, Clone, PartialEq)]
pub struct Conflict {
    /// The conflict target columns, empty when no target is given.
    pub target: Vec<&'static str>,
    /// The action executed on conflict.
    pub action: ConflictAction,
}

impl Conflict {
    /// Appends the clause to `sql`, binding `DO UPDATE` values to `params`.
    pub(crate) fn render<D: Dialect>(&self, sql: &mut String, params: &mut Vec<SqlVal>) {
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

        match &self.action {
            ConflictAction::DoNothing => sql.push_str(" DO NOTHING"),
            ConflictAction::DoUpdate(assignments) => {
                sql.push_str(" DO UPDATE SET ");
                for (index, (name, value)) in assignments.iter().enumerate() {
                    if index > 0 {
                        sql.push_str(", ");
                    }
                    D::push_quoted_identifier(sql, name);
                    sql.push_str(" = ");
                    value.render::<D>(sql, params);
                }
            }
        }
    }
}

/// An `ON CONFLICT` clause under construction for an [`InsertQuery`].
///
/// The conflict target is fixed by [`InsertQuery::on_conflict`], while
/// [`OnConflict::do_nothing`] and [`OnConflict::do_update`] choose the action and
/// hand the statement back for rendering.
pub struct OnConflict<D: Dialect, T: Table, Values, Returning> {
    pub(crate) query: InsertQuery<D, T, Values, Returning>,
    pub(crate) target: Vec<&'static str>,
}

impl<D, T, Values, Returning> OnConflict<D, T, Values, Returning>
where
    D: Dialect + SupportsOnConflict,
    T: Table,
{
    /// Completes the clause as `DO NOTHING`, skipping conflicting rows.
    pub fn do_nothing(self) -> InsertQuery<D, T, Values, Returning> {
        self.query.with_conflict(Conflict {
            target: self.target,
            action: ConflictAction::DoNothing,
        })
    }

    /// Completes the clause as `DO UPDATE SET ...` using `assignments`.
    ///
    /// The assignments are typed column assignments of the target table `T`, so a
    /// conflicting row can only be updated through columns of that table.
    pub fn do_update<A>(self, assignments: A) -> InsertQuery<D, T, Values, Returning>
    where
        A: AssignmentSet<T>,
    {
        let mut columns = Vec::new();
        assignments.extend_assignments(&mut columns);
        debug_assert!(
            !columns.is_empty(),
            "an ON CONFLICT DO UPDATE requires at least one column assignment"
        );

        self.query.with_conflict(Conflict {
            target: self.target,
            action: ConflictAction::DoUpdate(
                columns
                    .into_iter()
                    .map(|assignment| (assignment.column, assignment.value.clone()))
                    .collect(),
            ),
        })
    }
}
