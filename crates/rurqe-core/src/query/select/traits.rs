//! The compile-time assertions the `SELECT` typestates back.
//!
//! A trait here names a property of a builder state, so a method whose statement
//! would be invalid in that state simply is not available: the property is checked
//! while the query is typed instead of while the rendered SQL reaches the engine.

use super::{HasProjection, LockUnavailable, LockWaitSet, Locked, NoLock, SetOpApplied, TailClosed};

/// Statically asserts that a `SELECT` query records the rows it projects.
///
/// A query starts without projections, where rendering it would produce the
/// invalid `SELECT  FROM "users"` statement. Only
/// [`SelectQuery::select`](super::SelectQuery::select) transitions the query into
/// [`HasProjection`], the sole state implementing this trait, so
/// [`SelectQuery::build`](super::SelectQuery::build),
/// [`SelectQuery::to_ast`](super::SelectQuery::to_ast), and
/// [`SelectQuery::as_cte`](super::SelectQuery::as_cte) stay unreachable before a
/// projection is given — in debug and release builds alike.
///
/// ```compile_fail
/// use rurqe_core::dialect::Dialect;
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
/// rurqe_core::table! {
///     User => "users" {
///         UserId => "id": i64,
///     }
/// }
///
/// // Fails to compile: the query records no projection yet.
/// let _ = SelectQuery::<MockDialect>::new().from::<User>().build();
/// ```
#[diagnostic::on_unimplemented(
    message = "refusing to build a SELECT query without a projection",
    label = "this query does not project any row yet",
    note = "call `.select(...)` with the columns to project before building, rendering, or embedding the query"
)]
pub trait ProjectionPresent {}

impl ProjectionPresent for HasProjection {}

/// Statically asserts that a `SELECT` query may still record the rows it projects.
///
/// A statement records one projection list, and everything written around it —
/// the filter, the grouping, the ordering, and the set operations it takes part
/// in — is stated in terms of those output columns. The marker names the states
/// where that list may still be recorded:
///
/// - [`NoLock`] and [`Locked`], where the statement projects the rows of its own
///   `FROM` scope whether or not it claims them;
/// - [`LockWaitSet`], where the statement claimed them and fixed how it reacts to
///   rows locked elsewhere;
/// - [`LockUnavailable`], where deduplication changed how the rows are produced
///   without fixing them yet.
///
/// A statement a set operation combined carries [`SetOpApplied`](super::SetOpApplied),
/// and the ordering of its combined rows carries
/// [`TailClosed`](super::TailClosed): the rows of such a statement are the output
/// columns the first projection list named, so a second `select` would rebind what
/// the operands were never combined against.
#[diagnostic::on_unimplemented(
    message = "cannot select the rows of `{Self}`",
    label = "this statement already fixed the rows it produces",
    note = "`select` records the projection of a statement, so it is unavailable once `UNION`, `INTERSECT`, or `EXCEPT` combined the statement with another operand",
    note = "order the combined rows with `order_by_output` or `order_by_index` instead of re-projecting them"
)]
pub trait ProjectionOpen {}

impl ProjectionOpen for NoLock {}

impl ProjectionOpen for Locked {}

impl ProjectionOpen for LockWaitSet {}

impl ProjectionOpen for LockUnavailable {}

/// Statically asserts that a `SELECT` query may be combined with another statement.
///
/// A set operation compares the rows of several queries, so the combined
/// statement can neither keep the row-level lock of an operand nor accept a new
/// one: a query that already requests a lock carries [`Locked`] and does not
/// implement this trait.
///
/// A compound statement that already recorded the ordering of its combined rows
/// carries [`TailClosed`](super::TailClosed) for the same reason: SQL binds that
/// ordering to the whole result, so another operand would have to be combined
/// before it instead of after it.
///
/// ```compile_fail
/// use rurqe_core::dialect::Dialect;
/// use rurqe_core::query::{HasProjection, SelectQuery};
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
/// rurqe_core::table! {
///     User => "users" {
///         UserId => "id": i64,
///     }
/// }
///
/// fn base() -> SelectQuery<MockDialect, User, (i64,), HasProjection> {
///     SelectQuery::<MockDialect>::new()
///         .from::<User>()
///         .select((UserId,))
/// }
///
/// fn main() {
///     // Fails to compile: the ordering of the combined rows already exists, so no
///     // operand may follow it.
///     let _ = base()
///         .union(base())
///         .order_by_output_asc(UserId)
///         .union(base());
/// }
/// ```
#[diagnostic::on_unimplemented(
    message = "`{Self}` cannot be combined with another statement",
    label = "this statement requests a row-level lock or already ordered its combined rows",
    note = "remove the `FOR UPDATE` or `FOR SHARE` clause before combining the statement with `UNION`, `INTERSECT`, or `EXCEPT`",
    note = "a compound statement that already recorded the ordering of its combined rows takes no further operand, because SQL binds that ordering to the whole result"
)]
pub trait SetOperand {}

impl SetOperand for NoLock {}

impl SetOperand for LockUnavailable {}

impl SetOperand for SetOpApplied {}

/// Statically asserts that a `SELECT` query requests no row-level lock.
///
/// A row-level lock claims the rows a statement read for the transaction that runs
/// it, so it describes a statement the application executes itself. A statement
/// nested in a clause of another statement — a scalar operand, an `EXISTS` or `IN`
/// predicate, a `WITH` body, a derived table, a `LATERAL` subquery, or an operand
/// of a set operation — only supplies rows to that statement, so a lock written
/// there would belong to no statement of its own. The conversions that nest a
/// query therefore accept an unlocked query alone, which leaves `for_update` and
/// `for_share` as the only way to request a lock, while `to_ast` still describes a
/// locked statement and `build` still renders one.
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
///     let locked = SelectQuery::<MockDialect>::new()
///         .from::<Job>()
///         .select((JobId,))
///         .for_update();
///
///     // Fails to compile: the locking statement is a statement of its own, so it
///     // cannot become the nested statement of an `EXISTS` predicate.
///     let _ = locked.exists();
/// }
/// ```
///
/// A statement that requests no lock nests normally:
///
/// ```
/// use rurqe_core::dialect::Dialect;
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
/// rurqe_core::table! {
///     Job => "jobs" {
///         JobId => "id": i64,
///     }
/// }
///
/// let predicate = SelectQuery::<MockDialect>::new()
///     .from::<Job>()
///     .select((JobId,))
///     .exists();
///
/// assert!(matches!(predicate.kind(), rurqe_core::expr::ExprKind::Subquery { .. }));
/// ```
#[diagnostic::on_unimplemented(
    message = "`{Self}` requests a row-level lock, so it cannot be nested in another statement",
    label = "this statement locks the rows it selects",
    note = "a `FOR UPDATE` or `FOR SHARE` clause claims the rows a statement read for its own transaction, so a locking statement is executed on its own instead of being nested as a subquery, a `WITH` body, a derived table, a `LATERAL` subquery, or a set-operation operand",
    note = "drop the locking clause before nesting the query, or request the lock on the statement that executes the query"
)]
pub trait Unlocked {}

impl Unlocked for NoLock {}

impl Unlocked for LockUnavailable {}

impl Unlocked for SetOpApplied {}

impl Unlocked for TailClosed {}
