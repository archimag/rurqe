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
use crate::expr::{
    render_expr, DialectInScope, Expr, ExprKind, NonAggregate, RawSqlError, SqlVal, Unscoped,
};
use crate::query::{
    reject_unresolved_qualifiers, validate_expr, validate_row_clause, BuildError, RowClause,
};
use crate::schema::{Table, TableRef};

use super::insert::merge_assignments;
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
                if assignments.is_empty() {
                    sql.push_str(" DO NOTHING");
                } else {
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

// A predicate reading the inserted table, and one comparing two or three of its
// columns: a chain of `AND`-combined conditions folds through the `MergeScope`
// lattice, which appends every table it merges.
impl<T: Table> ConflictTargetScope<T> for T {}

impl<T: Table> ConflictTargetScope<T> for (T, T) {}

impl<T: Table> ConflictTargetScope<T> for (T, T, T) {}

/// Statically asserts that an update predicate reads the inserted table and the
/// row the statement proposed for it.
///
/// Comparing the stored row with the proposed one is the canonical conditional
/// update, so both relations are in scope of the trailing predicate.
///
/// The trait recognises the scopes a chain of up to three `AND`-combined
/// conditions builds from those two relations: `{T}`, `Excluded<T>`, the empty
/// scope, the dropped-scope marker, and the two- and three-element tuples the
/// [`MergeScope`](crate::expr::MergeScope) lattice folds them into. A longer
/// chain falls outside those bounds, so it does not compile instead of widening
/// the predicate past what the clause can resolve.
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

// A chain of three conditions folds its operands through the `MergeScope`
// lattice one at a time, so each association of the stored row and the proposed
// one produces a three-element scope. The proposed row is idempotent under
// merging, so a chain never carries it twice and `(Excluded<T>, Excluded<T>, ..)`
// has no form here.
impl<T: Table> ConflictUpdateScope<T> for (T, T, T) {}

impl<T: Table> ConflictUpdateScope<T> for (T, T, Excluded<T>) {}

impl<T: Table> ConflictUpdateScope<T> for (T, Excluded<T>, T) {}

impl<T: Table> ConflictUpdateScope<T> for (Excluded<T>, T, T) {}

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
        let mut incoming = Vec::new();
        assignments.into_assignments(&mut incoming);

        let mut deduped_assignments: Vec<(&'static str, AssignValue)> = Vec::new();
        merge_assignments(&mut deduped_assignments, incoming);

        let action = if deduped_assignments.is_empty() {
            ConflictAction::DoNothing
        } else {
            ConflictAction::DoUpdate {
                assignments: deduped_assignments,
                predicate: self.update_predicate,
            }
        };

        self.query.with_conflict(Conflict {
            target: self.target,
            target_predicate: self.target_predicate,
            action,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dml::{AssignColumn, ColumnAssign};
    use crate::expr::Expression;
    use core::fmt::Write;

    struct MockDialect;

    impl Dialect for MockDialect {
        fn push_quoted_identifier(out: &mut String, ident: &str) {
            out.push('"');
            out.push_str(ident);
            out.push('"');
        }

        fn push_placeholder(out: &mut String, index: usize) {
            let _ = write!(out, "${index}");
        }
    }

    impl SupportsOnConflict for MockDialect {}

    crate::table! {
        User => "users" {
            UserId => "id": i64,
            UserName => "name": String,
        }
    }

    struct EmptyAssignments;

    impl AssignmentSet<User> for EmptyAssignments {
        fn extend_assignments<'a>(&'a self, _out: &mut Vec<AssignColumn<'a>>) {}
        fn into_assignments(self, _out: &mut Vec<(&'static str, AssignValue)>) {}
    }

    impl AssignmentScope for EmptyAssignments {
        type Scope = ();
        type Dialect = ();
    }

    fn assert_target_scope<S: ConflictTargetScope<User>>() {}

    fn assert_update_scope<S: ConflictUpdateScope<User>>() {}

    #[test]
    fn the_target_predicate_reads_the_inserted_table() {
        assert_target_scope::<()>();
        assert_target_scope::<User>();
        assert_target_scope::<Unscoped>();

        // Comparing two columns of the inserted table reads the same table twice.
        assert_target_scope::<(User, User)>();
    }

    #[test]
    fn the_update_predicate_reads_the_proposed_row() {
        assert_update_scope::<()>();
        assert_update_scope::<User>();
        assert_update_scope::<Excluded<User>>();

        // A predicate comparing the stored row with the proposed one reads both,
        // whichever side of the comparison names them.
        assert_update_scope::<(User, Excluded<User>)>();
        assert_update_scope::<(Excluded<User>, User)>();
    }

    #[test]
    fn a_three_term_target_predicate_reads_the_inserted_table() {
        // Comparing three columns of the inserted table reads the same table
        // three times, which the lattice folds into one three-element scope.
        assert_target_scope::<(User, User, User)>();
    }

    #[test]
    fn three_term_update_predicates_keep_the_stored_and_proposed_rows() {
        // A chain of `AND`-combined conditions reads the stored row and the
        // proposed one side by side, whichever association the chain builds.
        assert_update_scope::<(User, User, User)>();
        assert_update_scope::<(User, User, Excluded<User>)>();
        assert_update_scope::<(User, Excluded<User>, User)>();
        assert_update_scope::<(Excluded<User>, User, User)>();
    }

    #[test]
    fn a_three_term_update_predicate_renders_both_relations() {
        use crate::dml::excluded;

        let built = InsertQuery::<MockDialect, User>::new()
            .values((UserId::set(1i64), UserName::set("root")))
            .on_conflict((UserId,))
            .update_where(
                UserName
                    .eq("stored")
                    .and(excluded(UserName).eq("proposed"))
                    .and(UserName.ne("root")),
            )
            .do_update(UserName::set("upserted"))
            .build().expect("valid query");

        assert_eq!(
            built.sql,
            "INSERT INTO \"users\" (\"id\", \"name\") VALUES ($1, $2) ON CONFLICT (\"id\") DO UPDATE SET \"name\" = $3 WHERE (((\"name\" = $4) AND (EXCLUDED.\"name\" = $5)) AND (\"name\" <> $6))"
        );
        assert_eq!(
            built.params,
            vec![
                SqlVal::I64(1),
                SqlVal::Text("root".to_owned()),
                SqlVal::Text("upserted".to_owned()),
                SqlVal::Text("stored".to_owned()),
                SqlVal::Text("proposed".to_owned()),
                SqlVal::Text("root".to_owned()),
            ]
        );
    }

    #[test]
    fn empty_assignment_set_degrades_to_do_nothing() {
        let built = InsertQuery::<MockDialect, User>::new()
            .values((UserId::set(1i64), UserName::set("root")))
            .on_conflict((UserId,))
            .do_update(EmptyAssignments)
            .build().expect("valid query");

        assert_eq!(
            built.sql,
            "INSERT INTO \"users\" (\"id\", \"name\") VALUES ($1, $2) ON CONFLICT (\"id\") DO NOTHING"
        );
    }

    #[test]
    fn empty_assignment_set_with_predicates_degrades_to_do_nothing() {
        let built = InsertQuery::<MockDialect, User>::new()
            .values((UserId::set(1i64), UserName::set("root")))
            .on_conflict((UserId,))
            .target_where(UserId.gt(0i64))
            .update_where(UserName.ne("root"))
            .do_update(EmptyAssignments)
            .build().expect("valid query");

        assert_eq!(
            built.sql,
            "INSERT INTO \"users\" (\"id\", \"name\") VALUES ($1, $2) ON CONFLICT (\"id\") WHERE (\"id\" > $3) DO NOTHING"
        );
    }
}
