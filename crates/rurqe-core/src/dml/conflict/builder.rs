//! Scope traits and the typestate builder of an `ON CONFLICT` clause.
//!
//! [`OnConflict`] assembles a clause fluently: the conflict target is fixed by
//! [`InsertQuery::on_conflict`](crate::dml::InsertQuery::on_conflict), the optional
//! predicates are recorded through
//! [`target_where`](OnConflict::target_where) and
//! [`update_where`](OnConflict::update_where), and the action is chosen through
//! [`do_nothing`](OnConflict::do_nothing) or
//! [`do_update`](OnConflict::do_update).
//!
//! The predicates are checked against the relations the clause can resolve at the
//! call site: [`ConflictTargetScope`] admits the inserted table alone, while
//! [`ConflictUpdateScope`] admits the inserted table and its proposed `EXCLUDED`
//! row.

use core::marker::PhantomData;

use crate::dialect::{Dialect, SupportsOnConflict};
use crate::dml::insert::merge_assignments;
use crate::dml::{AssignValue, AssignmentScope, AssignmentSet, Excluded, InsertQuery};
use crate::expr::{BinaryOp, DialectInScope, Expr, ExprKind, NonAggregate, Unscoped};
use crate::schema::Table;

use super::clause::{Conflict, ConflictAction};

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

/// Folds `incoming` into the predicate a clause already recorded.
///
/// Every predicate-recording builder of the crate accumulates its criteria —
/// [`SelectQuery::where_`](crate::query::SelectQuery::where_),
/// [`UpdateQuery::where_`](crate::dml::UpdateQuery::where_), and
/// [`Expr::filter_where`](crate::expr::Expr::filter_where) among them — so the
/// `ON CONFLICT` predicates follow the same rule: a repeated call narrows the
/// clause it was given instead of loosening what an earlier call stated.
fn and_recorded(recorded: Option<ExprKind>, incoming: ExprKind) -> ExprKind {
    match recorded {
        Some(existing) => ExprKind::Binary {
            op: BinaryOp::And,
            lhs: Box::new(existing),
            rhs: Box::new(incoming),
        },
        None => incoming,
    }
}

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
/// [`OnConflict::update_where`]. Both accumulate: a repeated call folds its
/// criterion into the recorded one with the SQL `AND` operator, so no
/// restriction is dropped when a conditional upsert chains them.
/// [`OnConflict::do_nothing`] and [`OnConflict::do_update`] then choose the
/// action and hand the statement back for rendering.
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
    /// rurqe_core::table!(
    ///     User => "users" {
    ///         UserId => "id": i64,
    ///     }
    /// );
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
    ///
    /// Repeated calls accumulate instead of replacing the recorded criterion: each
    /// predicate is folded into the previous one with the SQL `AND` operator, the
    /// same rule [`SelectQuery::where_`](crate::query::SelectQuery::where_) and
    /// [`Expr::filter_where`](crate::expr::Expr::filter_where) follow. Targeting
    /// several partial indexes therefore keeps every condition in force, and the
    /// rendered clause names all of them:
    ///
    /// ```text
    /// ON CONFLICT ("id") WHERE (("id" > $1) AND ("tenant_id" = $2)) DO UPDATE ...
    /// ```
    pub fn target_where<S, PredD>(mut self, predicate: Expr<bool, S, NonAggregate, PredD>) -> Self
    where
        S: ConflictTargetScope<T>,
        PredD: DialectInScope<D>,
    {
        self.target_predicate = Some(and_recorded(
            self.target_predicate.take(),
            predicate.into_kind(),
        ));
        self
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
    /// Repeated calls accumulate instead of replacing the recorded criterion: each
    /// predicate is folded into the previous one with the SQL `AND` operator, so a
    /// tenant restriction followed by a staleness check stays in force rather than
    /// being overwritten by the later call:
    ///
    /// ```text
    /// ... DO UPDATE SET "name" = $3
    ///     WHERE (("tenant_id" = $4) AND ("updated_at" < $5))
    /// ```
    ///
    /// Recording a predicate moves the clause into [`HasUpdatePredicate`], where
    /// [`do_nothing`](OnConflict::do_nothing) is no longer reachable, so the
    /// predicate cannot be dropped silently.
    pub fn update_where<S, PredD>(
        mut self,
        predicate: Expr<bool, S, NonAggregate, PredD>,
    ) -> OnConflict<D, T, Values, Returning, HasUpdatePredicate>
    where
        S: ConflictUpdateScope<T>,
        PredD: DialectInScope<D>,
    {
        self.update_predicate = Some(and_recorded(
            self.update_predicate.take(),
            predicate.into_kind(),
        ));
        OnConflict {
            query: self.query,
            target: self.target,
            target_predicate: self.target_predicate,
            update_predicate: self.update_predicate,
            _marker: PhantomData,
        }
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
    /// rurqe_core::table!(
    ///     User => "users" {
    ///         UserId => "id": i64,
    ///     }
    /// );
    ///
    /// rurqe_core::table!(
    ///     Account => "accounts" {
    ///         AccountId => "id": i64,
    ///     }
    /// );
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
