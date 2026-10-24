//! `ON CONFLICT` (upsert) clause model and its single-pass renderer.
//!
//! [`Conflict`] records the conflict target, the predicate deciding which rows
//! that target covers, and the action taken when the target is violated. The
//! clause is attached to an [`InsertQuery`] through
//! [`InsertQuery::on_conflict`] or [`InsertQuery::on_conflict_do_nothing`], both
//! of which require a dialect implementing [`SupportsOnConflict`], so a dialect
//! without upsert support simply does not compile.
//!
//! ```text
//! INSERT INTO "users" ("id", "name") VALUES ($1, $2)
//!     ON CONFLICT ("id") WHERE ("id" > $3)
//!     DO UPDATE SET "name" = EXCLUDED."name" WHERE ("name" <> $4)
//! ```
//!
//! The leading predicate restricts the target to the rows a partial unique index
//! covers, while the trailing one restricts the update to the conflicting rows
//! worth rewriting. Both are checked at the call site against the relations their
//! clause can read, so a predicate naming a table outside the statement does not
//! compile.

use core::marker::PhantomData;

use crate::dialect::{Dialect, SupportsOnConflict};
use crate::expr::{render_expr, DialectInScope, Expr, ExprKind, NonAggregate, SqlVal, Unscoped};
use crate::schema::Table;

use super::{AssignValue, AssignmentScope, AssignmentSet, Excluded, InsertQuery};

/// The action executed when an inserted row violates the conflict target.
#[derive(Debug, Clone, PartialEq)]
pub enum ConflictAction {
    /// `DO NOTHING`, silently skipping the conflicting row.
    DoNothing,
    /// `DO UPDATE SET ...`, applying typed assignments to the existing row.
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
    /// Appends the clause to `sql`, binding every value to `params`.
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

        // The target predicate selects the index the statement conflicts on, so
        // it closes the target before the action follows. Both predicates address
        // the columns of the inserted table unqualified, which every dialect
        // accepts, while the proposed row names its own `EXCLUDED` relation.
        if let Some(predicate) = &self.target_predicate {
            sql.push_str(" WHERE ");
            render_expr::<D>(sql, params, predicate, &[]);
        }

        match &self.action {
            ConflictAction::DoNothing => sql.push_str(" DO NOTHING"),
            ConflictAction::DoUpdate {
                assignments,
                predicate,
            } => {
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
                    value.render::<D>(sql, params, &[]);
                }
                if let Some(predicate) = predicate {
                    sql.push_str(" WHERE ");
                    render_expr::<D>(sql, params, predicate, &[]);
                }
            }
        }
    }
}

/// Statically asserts that a conflict target predicate only reads the table the
/// row is inserted into.
///
/// The target predicate selects the index the statement conflicts on, which the
/// engine evaluates before the conflict exists, so the proposed row of
/// `EXCLUDED` stays unreachable from it.
#[diagnostic::on_unimplemented(
    message = "the conflict target predicate of table `{T}` reads a relation it cannot reach",
    label = "only columns of `{T}` are in scope of the conflict target",
    note = "restrict the target with columns of `{T}`; the proposed row is reachable from the update predicate only"
)]
pub trait ConflictTargetScope<T: Table> {}

// A constant predicate reads no relation, and so does one whose scope was dropped
// explicitly through `without_scope`.
impl<T: Table> ConflictTargetScope<T> for () {}

impl<T: Table> ConflictTargetScope<T> for Unscoped {}

// A predicate reading the inserted table, and one comparing two of its columns.
impl<T: Table> ConflictTargetScope<T> for T {}

impl<T: Table> ConflictTargetScope<T> for (T, T) {}

/// Statically asserts that an update predicate reads the inserted table and the
/// row the statement proposed for it.
///
/// Comparing the stored row with the proposed one is the canonical conditional
/// update, so both relations are in scope of the trailing predicate.
#[diagnostic::on_unimplemented(
    message = "the update predicate of table `{T}` reads a relation it cannot reach",
    label = "only columns of `{T}` and its `EXCLUDED` row are in scope of the update predicate",
    note = "compare the stored row with the proposed one through `excluded(Column)`"
)]
pub trait ConflictUpdateScope<T: Table> {}

impl<T: Table> ConflictUpdateScope<T> for () {}

impl<T: Table> ConflictUpdateScope<T> for Unscoped {}

impl<T: Table> ConflictUpdateScope<T> for T {}

impl<T: Table> ConflictUpdateScope<T> for Excluded<T> {}

impl<T: Table> ConflictUpdateScope<T> for (T, T) {}

impl<T: Table> ConflictUpdateScope<T> for (T, Excluded<T>) {}

impl<T: Table> ConflictUpdateScope<T> for (Excluded<T>, T) {}

/// Marker of an `ON CONFLICT` clause that records no update predicate yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NoUpdatePredicate;

/// Marker of an `ON CONFLICT` clause whose update branch is restricted.
///
/// The state is reached through [`OnConflict::update_where`], the only builder of
/// the marker, so a `DO NOTHING` clause can never be built from a clause that
/// records a predicate it would silently drop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HasUpdatePredicate;

/// Phantom marker carrying the update predicate typestate of an [`OnConflict`] clause.
type ConflictMarker<Predicate> = PhantomData<fn() -> Predicate>;

/// An `ON CONFLICT` clause under construction for an [`InsertQuery`].
///
/// The conflict target is fixed by [`InsertQuery::on_conflict`], while the
/// optional predicates are added through [`OnConflict::target_where`] and
/// [`OnConflict::update_where`]. [`OnConflict::do_nothing`] and
/// [`OnConflict::do_update`] then choose the action and hand the statement back
/// for rendering.
pub struct OnConflict<D: Dialect, T: Table, Values, Returning, Predicate = NoUpdatePredicate> {
    query: InsertQuery<D, T, Values, Returning>,
    target: Vec<&'static str>,
    target_predicate: Option<ExprKind>,
    update_predicate: Option<ExprKind>,
    _marker: ConflictMarker<Predicate>,
}

impl<D, T, Values, Returning> OnConflict<D, T, Values, Returning, NoUpdatePredicate>
where
    D: Dialect + SupportsOnConflict,
    T: Table,
{
    /// Starts the `ON CONFLICT` clause of `query` on the target `columns`.
    pub(crate) fn new(
        query: InsertQuery<D, T, Values, Returning>,
        target: Vec<&'static str>,
    ) -> Self {
        OnConflict {
            query,
            target,
            target_predicate: None,
            update_predicate: None,
            _marker: PhantomData,
        }
    }

    /// Restricts the rows the `DO UPDATE` branch rewrites.
    ///
    /// The predicate is rendered after the assignments of the branch, so it may
    /// compare the stored row with the one the statement proposed:
    ///
    /// ```text
    /// DO UPDATE SET "name" = EXCLUDED."name" WHERE ("name" <> EXCLUDED."name")
    /// ```
    ///
    /// Recording a predicate moves the clause into [`HasUpdatePredicate`], where
    /// [`do_nothing`](OnConflict::do_nothing) is no longer reachable, so the
    /// predicate cannot be dropped silently.
    pub fn update_where<S, PredD>(
        self,
        predicate: Expr<bool, S, NonAggregate, PredD>,
    ) -> OnConflict<D, T, Values, Returning, HasUpdatePredicate>
    where
        S: ConflictUpdateScope<T>,
        PredD: DialectInScope<D>,
    {
        OnConflict {
            query: self.query,
            target: self.target,
            target_predicate: self.target_predicate,
            update_predicate: Some(predicate.into_kind()),
            _marker: PhantomData,
        }
    }

    /// Completes the clause as `DO NOTHING`, skipping conflicting rows.
    ///
    /// The method stays unreachable once an update predicate was recorded, since
    /// the clause it would build has no branch the predicate could apply to:
    ///
    /// ```compile_fail
    /// use rurqe_core::dialect::{Dialect, SupportsOnConflict};
    /// use rurqe_core::dml::{ColumnAssign, InsertQuery};
    /// use rurqe_core::expr::Expr;
    ///
    /// struct MockDialect;
    ///
    /// impl Dialect for MockDialect {
    ///     fn push_quoted_identifier(out: &mut String, ident: &str) {
    ///         out.push_str(ident);
    ///     }
    ///
    ///     fn push_placeholder(out: &mut String, index: usize) {
    ///         let _ = index;
    ///     }
    /// }
    ///
    /// impl SupportsOnConflict for MockDialect {}
    ///
    /// rurqe_core::table! {
    ///     User => "users" {
    ///         UserId => "id": i64,
    ///     }
    /// }
    ///
    /// // Fails to compile: the recorded update predicate cannot apply to a
    /// // `DO NOTHING` clause.
    /// let _ = InsertQuery::<MockDialect, User>::new()
    ///     .values(UserId::set(1i64))
    ///     .on_conflict((UserId,))
    ///     .update_where(Expr::column::<UserId>().gt(1i64))
    ///     .do_nothing();
    /// ```
    pub fn do_nothing(self) -> InsertQuery<D, T, Values, Returning> {
        self.query.with_conflict(Conflict {
            target: self.target,
            target_predicate: self.target_predicate,
            action: ConflictAction::DoNothing,
        })
    }
}

impl<D, T, Values, Returning, Predicate> OnConflict<D, T, Values, Returning, Predicate>
where
    D: Dialect + SupportsOnConflict,
    T: Table,
{
    /// Restricts the conflict target to the rows the partial unique index covers.
    ///
    /// The predicate selects the index the statement conflicts on, so the dialect
    /// renders it between the target columns and the action:
    ///
    /// ```text
    /// ON CONFLICT ("id") WHERE ("id" > $1) DO UPDATE SET ...
    /// ```
    pub fn target_where<S, PredD>(mut self, predicate: Expr<bool, S, NonAggregate, PredD>) -> Self
    where
        S: ConflictTargetScope<T>,
        PredD: DialectInScope<D>,
    {
        self.target_predicate = Some(predicate.into_kind());
        self
    }

    /// Completes the clause as `DO UPDATE SET ...` using `assignments`.
    ///
    /// The assignments are typed column assignments of the target table `T`, so a
    /// conflicting row can only be updated through columns of that table.
    ///
    /// The branch addresses the stored row of `T` and the row the statement
    /// proposed for it (`EXCLUDED`), so an expression assignment may read exactly
    /// those two relations — the same rule
    /// [`update_where`](OnConflict::update_where) follows. Reading any other
    /// table would render a reference the upsert branch cannot resolve, so it is
    /// rejected at compile time:
    ///
    /// ```compile_fail
    /// use rurqe_core::dialect::{Dialect, SupportsOnConflict};
    /// use rurqe_core::dml::{ColumnAssign, InsertQuery};
    /// use rurqe_core::expr::Expr;
    ///
    /// struct MockDialect;
    ///
    /// impl Dialect for MockDialect {
    ///     fn push_quoted_identifier(out: &mut String, ident: &str) {
    ///         out.push_str(ident);
    ///     }
    ///
    ///     fn push_placeholder(out: &mut String, index: usize) {
    ///         let _ = index;
    ///     }
    /// }
    ///
    /// impl SupportsOnConflict for MockDialect {}
    ///
    /// rurqe_core::table! {
    ///     User => "users" {
    ///         UserId => "id": i64,
    ///     }
    /// }
    ///
    /// rurqe_core::table! {
    ///     Account => "accounts" {
    ///         AccountId => "id": i64,
    ///     }
    /// }
    ///
    /// // Fails to compile: the branch reads `Account`, which it cannot reach.
    /// let _ = InsertQuery::<MockDialect, User>::new()
    ///     .values(UserId::set(1i64))
    ///     .on_conflict((UserId,))
    ///     .do_update(UserId::set_expr(Expr::column::<AccountId>()));
    /// ```
    pub fn do_update<A>(self, assignments: A) -> InsertQuery<D, T, Values, Returning>
    where
        A: AssignmentSet<T> + AssignmentScope,
        <A as AssignmentScope>::Scope: ConflictUpdateScope<T>,
        <A as AssignmentScope>::Dialect: DialectInScope<D>,
    {
        let mut columns = Vec::new();
        assignments.extend_assignments(&mut columns);
        debug_assert!(
            !columns.is_empty(),
            "an ON CONFLICT DO UPDATE requires at least one column assignment"
        );

        let mut deduped_assignments: Vec<(&'static str, AssignValue)> = Vec::new();
        for item in columns {
            if let Some(existing) = deduped_assignments
                .iter_mut()
                .find(|(col, _)| *col == item.column)
            {
                existing.1 = item.value.clone();
            } else {
                deduped_assignments.push((item.column, item.value.clone()));
            }
        }

        self.query.with_conflict(Conflict {
            target: self.target,
            target_predicate: self.target_predicate,
            action: ConflictAction::DoUpdate {
                assignments: deduped_assignments,
                predicate: self.update_predicate,
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::expr::Merged;

    crate::table! {
        User => "users" {
            UserId => "id": i64,
            UserName => "name": String,
        }
    }

    fn assert_target_scope<S: ConflictTargetScope<User>>() {}

    fn assert_update_scope<S: ConflictUpdateScope<User>>() {}

    #[test]
    fn the_target_predicate_reads_the_inserted_table() {
        assert_target_scope::<()>();
        assert_target_scope::<User>();
        assert_target_scope::<Unscoped>();

        // Comparing two columns of the inserted table reads the same table twice.
        assert_target_scope::<Merged<User, User>>();
    }

    #[test]
    fn the_update_predicate_reads_the_proposed_row() {
        assert_update_scope::<()>();
        assert_update_scope::<User>();
        assert_update_scope::<Excluded<User>>();

        // A predicate comparing the stored row with the proposed one reads both,
        // whichever side of the comparison names them.
        assert_update_scope::<Merged<User, Excluded<User>>>();
        assert_update_scope::<Merged<Excluded<User>, User>>();
    }
}
