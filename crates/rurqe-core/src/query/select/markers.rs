//! The typestate markers a `SELECT` statement passes through.
//!
//! A marker carries no data: it names a state a builder chain can reach, so the
//! methods available on a query follow from the state the query records instead of
//! from a runtime check.

/// Typestate marker indicating that a `SELECT` query records no projection yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NoProjection;

/// Typestate marker indicating that a `SELECT` query records the rows it projects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HasProjection;

/// Typestate marker indicating that a `SELECT` query requests no row-level lock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NoLock;

/// Typestate marker indicating that a `SELECT` query requests a row-level lock.
///
/// The state is reached through [`SelectQuery::for_update`](super::SelectQuery::for_update)
/// or [`SelectQuery::for_share`](super::SelectQuery::for_share), the only builders
/// of the marker, so a locking clause can neither be requested twice nor modified
/// before it exists.
///
/// The closing wait mode is reachable from this state alone: both
/// [`SelectQuery::nowait`](super::SelectQuery::nowait) and
/// [`SelectQuery::skip_locked`](super::SelectQuery::skip_locked) are defined on it
/// and each of them moves the query into [`LockWaitSet`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Locked;

/// Typestate marker indicating that a locked `SELECT` query fixed how it reacts to rows locked elsewhere.
///
/// A row-level lock carries at most one closing modifier, and SQL refuses to
/// combine them: PostgreSQL answers `FOR UPDATE NOWAIT SKIP LOCKED` with `NOWAIT
/// and SKIP LOCKED cannot be specified together`. The builder therefore records
/// the modifier once — `nowait` and `skip_locked` are reachable from [`Locked`]
/// alone, and each of them moves the query here, where neither of them is
/// available any more. The exclusion is thus a property of the typestate rather
/// than of a runtime check, so no build chain spells a clause the engine refuses:
///
/// ```compile_fail
/// use rurqe_core::dialect::{Dialect, SupportsRowLocking};
/// use rurqe_core::query::SelectQuery;
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
/// impl SupportsRowLocking for MockDialect {}
///
/// rurqe_core::table! {
///     Job => "jobs" {
///         JobId => "id": i64,
///     }
/// }
///
/// fn main() {
///     // Fails to compile: the lock already fixed its wait behaviour, so it cannot
///     // also skip the rows locked elsewhere.
///     let _ = SelectQuery::<MockDialect>::new()
///         .from::<Job>()
///         .select((JobId,))
///         .for_update()
///         .nowait()
///         .skip_locked();
/// }
/// ```
///
/// The mirrored order — `skip_locked()` followed by `nowait()` — is rejected the
/// same way, because both modifiers are reachable from [`Locked`] alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LockWaitSet;

/// Typestate marker indicating that a `SELECT` query cannot request a row-level lock.
///
/// PostgreSQL refuses `FOR UPDATE` and `FOR SHARE` on a statement that
/// deduplicates its rows (`DISTINCT`, `DISTINCT ON`), so recording the clause
/// moves the query out of [`NoLock`] and leaves the locking builders unreachable.
/// Grouping keeps a query out of the locking path as well: a grouped statement
/// produces one row per group, so it never becomes a plain
/// [`SelectQuery`](super::SelectQuery) again and carries no lock along the way.
///
/// A statement a set operation combined drops its lock for the same reason and
/// carries [`SetOpApplied`] instead, which moves the query into the
/// [`Compound`](crate::query::Compound) scope as well: the ordering of the
/// combined rows names output columns and positions instead of relations in
/// scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LockUnavailable;

/// Typestate marker indicating that a `SELECT` query combined its rows with another statement.
///
/// `UNION`, `INTERSECT`, and `EXCEPT` fix the rows the combined statement
/// produces: its output columns are the ones the first projection list names, and
/// every operand was combined on the understanding that it projects the same
/// arity. The statement the combination left behind therefore records its
/// projection for good — [`ProjectionOpen`](super::ProjectionOpen) stays
/// unimplemented for it, so
/// [`SelectQuery::select`](super::SelectQuery::select) is unreachable and the rows
/// of the combination cannot be rebound to a projection the operands were never
/// checked against:
///
/// ```text
/// first.union(second).select((other_column,)) // rejected
/// ```
///
/// The state carries two further consequences, mirroring the lock the combination
/// drops. The combined statement can neither request a row-level lock, so
/// [`SelectQuery::for_update`](super::SelectQuery::for_update) and
/// [`SelectQuery::for_share`](super::SelectQuery::for_share) stay unreachable,
/// nor keep combining a locked operand. The ordering of the combined rows goes
/// through [`SelectQuery::order_by_output`](super::SelectQuery::order_by_output)
/// and [`SelectQuery::order_by_index`](super::SelectQuery::order_by_index), either
/// of which closes the statement for further operands through [`TailClosed`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SetOpApplied;

/// Typestate marker indicating that a compound statement recorded its ordering tail.
///
/// A statement combined with `UNION`, `INTERSECT`, or `EXCEPT` sorts the rows the
/// combination produced, so its ordering belongs to the whole result rather than
/// to one operand. SQL binds it there: no operand can follow it, and
/// `A UNION B ORDER BY ... UNION C` is rejected by the engine.
///
/// The state is reached from the set-operation path — `order_by_index` and
/// `order_by_output` move the query here — and it stays reachable for every
/// further criterion, so a compound statement still orders by as many output
/// columns as it projects. What the state rules out is another operand:
/// [`SetOperand`](super::SetOperand) has no implementation for it, which leaves
/// `union`, `union_all`, `intersect`, and `except` unreachable.
///
/// ```text
/// first.union(second).order_by_index_asc(1).union(third) // rejected
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TailClosed;
