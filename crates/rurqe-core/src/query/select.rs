//! The type-safe `SELECT` builder and its single-pass SQL renderer.

use core::marker::PhantomData;

use crate::dialect::{Dialect, SupportsDistinctOn};
use crate::expr::{
    BinaryOp, CteAst, DerivedAst, DialectInScope, Expr, ExprKind, JoinAst, LockClause,
    NonAggregate, SqlVal,
};
use crate::schema::{Column, ExprInScope, Table, TableInScope, TableRef};

// `ORDER BY` criteria describe the statement AST shared with the expression
// module, so the canonical definitions live there and are re-exported here.
pub use crate::expr::{NullsOrder, OrderCriterion, OrderDirection};

use super::cte::{Cte, DerivedTable};
use super::group_by::{EmptyGroupSet, GroupBy, GroupByColumns, GroupByInScope, GroupedQuery};
use super::scope::{ScopePositions, TableScope};
use super::selection::{AggregateInDialect, Nullable, Selection, SelectionFor};
use super::set_ops::SetOpAst;

// The builder is decomposed by responsibility: joins establish the relational
// scope, ordering and pagination move rows of the clause they are written on,
// locking and set operations change the typestate of the statement, the
// transitions module names every typestate change a builder may make, and the
// rendering adapters convert the query into its statement AST.
mod joins;
mod locking;
mod ordering;
mod render;
mod set_ops;
mod transitions;

pub use self::ordering::OrderBy;

/// The rendered SQL text of a query together with its bound parameters.
#[derive(Debug, Clone, PartialEq)]
pub struct BuiltQuery {
    /// The SQL text using the dialect's positional placeholders.
    pub sql: String,

    /// The parameter values referenced by the placeholders, in placeholder order.
    pub params: Vec<SqlVal>,
}

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
/// The state is reached through [`SelectQuery::for_update`] or
/// [`SelectQuery::for_share`], the only builders of the marker, so a locking
/// clause can neither be requested twice nor modified before it exists.
///
/// The closing wait mode is reachable from this state alone: both
/// [`SelectQuery::nowait`] and [`SelectQuery::skip_locked`] are defined on it and
/// each of them moves the query into [`LockWaitSet`].
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
/// produces one row per group, so it never becomes a plain [`SelectQuery`] again
/// and carries no lock along the way.
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
/// projection for good — [`ProjectionOpen`] stays unimplemented for it, so
/// [`SelectQuery::select`] is unreachable and the rows of the combination cannot
/// be rebound to a projection the operands were never checked against:
///
/// ```text
/// first.union(second).select((other_column,)) // rejected
/// ```
///
/// The state carries two further consequences, mirroring the lock the combination
/// drops. The combined statement can neither request a row-level lock, so
/// [`SelectQuery::for_update`] and [`SelectQuery::for_share`] stay unreachable,
/// nor keep combining a locked operand. The ordering of the combined rows goes
/// through [`SelectQuery::order_by_output`] and
/// [`SelectQuery::order_by_index`], either of which closes the statement for
/// further operands through [`TailClosed`].
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
/// [`SetOperand`] has no implementation for it, which leaves `union`,
/// `union_all`, `intersect`, and `except` unreachable.
///
/// ```text
/// first.union(second).order_by_index_asc(1).union(third) // rejected
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TailClosed;

/// Statically asserts that a `SELECT` query records the rows it projects.
///
/// A query starts without projections, where rendering it would produce the
/// invalid `SELECT  FROM "users"` statement. Only [`SelectQuery::select`]
/// transitions the query into [`HasProjection`], the sole state implementing this
/// trait, so [`SelectQuery::build`], [`SelectQuery::to_ast`], and
/// [`SelectQuery::as_cte`] stay unreachable before a projection is given — in
/// debug and release builds alike.
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
/// A statement a set operation combined carries [`SetOpApplied`], and the
/// ordering of its combined rows carries [`TailClosed`]: the rows of such a
/// statement are the output columns the first projection list named, so a second
/// `select` would rebind what the operands were never combined against.
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
/// carries [`TailClosed`] for the same reason: SQL binds that ordering to the
/// whole result, so another operand would have to be combined before it instead
/// of after it.
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

/// A `SELECT` query that projects its rows and requests no row-level lock.
///
/// The alias names the everyday shape of a rendered statement, hiding the
/// projection and lock typestates [`SelectQuery`] carries: every query reaches
/// [`HasProjection`] through [`SelectQuery::select`] and keeps [`NoLock`] unless
/// it requests a row-level lock.
///
/// ```text
/// let query: Query<Postgres, Users, (i64, String)> = ...
/// ```
pub type Query<D, Tables = (), Output = ()> = SelectQuery<D, Tables, Output, HasProjection, NoLock>;

/// A `SELECT` query that projects its rows and requests a row-level lock.
///
/// The alias names a statement inside a transaction that claims its rows until
/// the transaction ends, hiding the projection and lock typestates
/// [`SelectQuery`] carries: it is reached after [`SelectQuery::for_update`] or
/// [`SelectQuery::for_share`] moved the query into [`Locked`].
///
/// ```text
/// let query: LockedQuery<Postgres, Users, (i64,)> = ...
/// ```
pub type LockedQuery<D, Tables = (), Output = ()> =
    SelectQuery<D, Tables, Output, HasProjection, Locked>;

/// Phantom marker carrying the dialect, scope, row type, and typestates of a query.
type QueryMarker<D, Tables, Output, Projected, Lock> =
    PhantomData<fn() -> (D, Tables, Output, Projected, Lock)>;

/// A `SELECT` query over the table `Tables` producing rows of type `Output`.
///
/// Every clause is accumulated as dialect-neutral statement data, so building a
/// query allocates no rendering closure and rendering it is a single pass over
/// the recorded statement. The ordering and pagination of a query chained with a
/// set operation are recorded separately: they apply to the combined rows rather
/// than to the left query.
///
/// `Projected` records whether the query already projects the rows it selects: a
/// fresh query carries [`NoProjection`], and [`SelectQuery::select`] moves it into
/// [`HasProjection`], the only state whose queries can be rendered.
///
/// `Lock` records whether the query requests a row-level lock: a plain query
/// carries [`NoLock`], and [`SelectQuery::for_update`] or
/// [`SelectQuery::for_share`] move it into [`Locked`], where the closing
/// [`SelectQuery::nowait`] and [`SelectQuery::skip_locked`] modifiers stay
/// reachable. A query that cannot be locked carries [`LockUnavailable`] instead:
/// deduplicating its rows with [`SelectQuery::distinct`] or
/// [`SelectQuery::distinct_on`], or grouping it, moves it out of [`NoLock`],
/// because PostgreSQL refuses a row-level lock on such a statement. Combining the
/// query with a set operation reaches [`SetOpApplied`] instead, which drops the
/// lock for the same reason while also fixing the rows the combined statement
/// produces.
pub struct SelectQuery<
    D: Dialect,
    Tables = (),
    Output = (),
    Projected = NoProjection,
    Lock = NoLock,
> {
    ctes: Vec<CteAst>,
    from: Option<TableRef>,
    projections: Vec<ExprKind>,
    joins: Vec<JoinAst>,
    derived: Vec<DerivedAst>,
    filter: Option<ExprKind>,
    pub(crate) group_by: Vec<ExprKind>,
    having: Option<ExprKind>,
    order_by: Vec<OrderCriterion>,
    limit: Option<u32>,
    offset: Option<u32>,
    set_operations: Vec<SetOpAst>,
    distinct: bool,
    distinct_on: Vec<ExprKind>,
    compound_order_by: Vec<OrderCriterion>,
    compound_limit: Option<u32>,
    compound_offset: Option<u32>,
    lock: Option<LockClause>,
    _marker: QueryMarker<D, Tables, Output, Projected, Lock>,
}

impl<D: Dialect, Tables, Output, Projected, Lock> Clone
    for SelectQuery<D, Tables, Output, Projected, Lock>
{
    fn clone(&self) -> Self {
        Self {
            ctes: self.ctes.clone(),
            from: self.from,
            projections: self.projections.clone(),
            joins: self.joins.clone(),
            derived: self.derived.clone(),
            filter: self.filter.clone(),
            group_by: self.group_by.clone(),
            having: self.having.clone(),
            order_by: self.order_by.clone(),
            limit: self.limit,
            offset: self.offset,
            set_operations: self.set_operations.clone(),
            distinct: self.distinct,
            distinct_on: self.distinct_on.clone(),
            compound_order_by: self.compound_order_by.clone(),
            compound_limit: self.compound_limit,
            compound_offset: self.compound_offset,
            lock: self.lock,
            _marker: PhantomData,
        }
    }
}

impl<D: Dialect> SelectQuery<D, (), (), NoProjection> {
    /// Creates a query without a table, projections, filter, or pagination.
    pub fn new() -> Self {
        SelectQuery {
            ctes: Vec::new(),
            from: None,
            projections: Vec::new(),
            joins: Vec::new(),
            derived: Vec::new(),
            filter: None,
            group_by: Vec::new(),
            having: None,
            order_by: Vec::new(),
            limit: None,
            offset: None,
            set_operations: Vec::new(),
            distinct: false,
            distinct_on: Vec::new(),
            compound_order_by: Vec::new(),
            compound_limit: None,
            compound_offset: None,
            lock: None,
            _marker: PhantomData,
        }
    }
}

impl<D: Dialect> Default for SelectQuery<D, (), (), NoProjection> {
    fn default() -> Self {
        Self::new()
    }
}

// A statement introduces its relation exactly once, so the builders that record
// the `FROM` clause are defined on the query whose scope is still empty — the
// same restriction `group_by` applies to its own typestate, and the reason the
// three `compile_fail` examples on `from` and `from_table` are backed by the
// signature instead of by prose alone. Widening this header back to the generic
// scope would let a second introduction replace the recorded `FROM` while the
// joins qualified against it survive, producing a statement whose rendered
// relations no longer match the scope the builder chain claims.
impl<D: Dialect, Output, Projected> SelectQuery<D, (), Output, Projected> {
    /// Introduces `T` as the table rows are selected from.
    ///
    /// A statement introduces its relation once: the method is defined on the query
    /// that carries no relation yet, so introducing a second one replaces neither
    /// the relation already recorded nor the joins that qualify against it.
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
    /// rurqe_core::table! {
    ///     Order => "orders" {
    ///         OrderId => "id": i64,
    ///     }
    /// }
    ///
    /// fn main() {
    ///     // Fails to compile: the query already introduces `User`, so a second
    ///     // relation cannot replace it.
    ///     let _ = SelectQuery::<MockDialect>::new()
    ///         .from::<User>()
    ///         .from::<Order>();
    /// }
    /// ```
    pub fn from<T: Table>(mut self) -> SelectQuery<D, T, Output, Projected> {
        self.from = Some(T::table_ref());
        self.with_scope::<T>()
    }

    /// Introduces `table` as the table rows are selected from.
    ///
    /// The value-based form of [`from`](Self::from) is restricted in the same way:
    /// it is defined on the query that carries no relation yet, so the relation a
    /// statement selects from cannot be replaced once it exists — neither by a
    /// second table nor after a join recorded against the relation already there.
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
    /// rurqe_core::table! {
    ///     Order => "orders" {
    ///         OrderId => "id": i64,
    ///     }
    /// }
    ///
    /// fn main() {
    ///     // Fails to compile: the second call would discard `User` from the
    ///     // statement while the query type still names it.
    ///     let _ = SelectQuery::<MockDialect>::new()
    ///         .from_table(User)
    ///         .from_table(Order);
    /// }
    /// ```
    ///
    /// ```compile_fail
    /// use rurqe_core::dialect::Dialect;
    /// use rurqe_core::expr::Expr;
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
    /// rurqe_core::table! {
    ///     Order => "orders" {
    ///         OrderId => "id": i64,
    ///         OrderUserId => "user_id": i64,
    ///     }
    /// }
    ///
    /// fn main() {
    ///     // Fails to compile: the join already introduced `orders` against
    ///     // `users`, so replacing the relation afterwards would leave the `ON`
    ///     // predicate qualifying a table the statement no longer declares.
    ///     let _ = SelectQuery::<MockDialect>::new()
    ///         .from_table(User)
    ///         .inner_join_table(
    ///             Order,
    ///             Expr::column::<UserId>().eq(Expr::column::<OrderUserId>()),
    ///         )
    ///         .from_table(Order);
    /// }
    /// ```
    pub fn from_table<T: Table>(mut self, _table: T) -> SelectQuery<D, T, Output, Projected> {
        self.from = Some(T::table_ref());
        self.with_scope::<T>()
    }

    /// Introduces the derived table `derived` as the relation rows are selected from.
    ///
    /// The query the derived table wraps is rendered inline as
    /// `(SELECT ...) AS "name"`, so the statement selects from its rows without a
    /// temporary relation, while the columns of the subquery are referenced
    /// through the `table!` marker the derived table was built with.
    pub fn from_subquery<T: Table, Row, SubD>(
        mut self,
        derived: DerivedTable<T, Row, SubD>,
    ) -> SelectQuery<D, T, Output, Projected>
    where
        SubD: DialectInScope<D>,
    {
        self.from = Some(T::table_ref());
        self.derived.push(derived.into_ast());
        self.with_scope::<T>()
    }
}

impl<D: Dialect, Tables> SelectQuery<D, Tables, (), NoProjection, NoLock> {
    /// Groups query results by `columns`, moving the query into the
    /// [`GroupedQuery`] state.
    ///
    /// `group_by` must be established before projections are selected so that
    /// every projected column is verified to be either grouped or an aggregate.
    /// The grouped query is not lockable: PostgreSQL evaluates a row-level lock on
    /// the rows the statement produced, while a grouped statement produces one row
    /// per group, so the [`GroupedQuery`] state keeps the locking builders out of
    /// reach.
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
    /// // Fails to compile: calling `group_by` after `select` is rejected.
    /// let _ = SelectQuery::<MockDialect>::new()
    ///     .from::<User>()
    ///     .select((UserId,))
    ///     .group_by(UserId);
    /// ```
    pub fn group_by<G, Positions>(
        mut self,
        columns: G,
    ) -> GroupedQuery<D, Tables, (), <G as GroupByColumns<EmptyGroupSet>>::Columns, NoProjection>
    where
        G: GroupBy + GroupByInScope<Tables, Positions> + GroupByColumns<EmptyGroupSet>,
    {
        columns.push_column_names(&mut self.group_by);
        GroupedQuery {
            query: self,
            _marker: PhantomData,
        }
    }
}

impl<D: Dialect, Tables, Output, Projected> SelectQuery<D, Tables, Output, Projected, NoLock> {
    /// Marks the query as `DISTINCT`, removing duplicate rows.
    ///
    /// Every expression of the `ORDER BY` clause of a `DISTINCT` statement has to
    /// appear in the projection list as well, because the engine sorts the rows it
    /// produced rather than the rows it read; PostgreSQL rejects the statement
    /// otherwise. The rule compares the assembled expressions rather than their
    /// types, so it is validated during `.build()` and surfaces as
    /// [`BuildError::DistinctOrderNotProjected`](crate::query::BuildError::DistinctOrderNotProjected);
    /// the related `DISTINCT ON (...)` prefix rule is validated there as well and
    /// surfaces as
    /// [`BuildError::DistinctOnOrderMismatch`](crate::query::BuildError::DistinctOnOrderMismatch).
    ///
    /// A deduplicated statement cannot request a row-level lock, so the query
    /// moves out of [`NoLock`] and [`SelectQuery::for_update`] and
    /// [`SelectQuery::for_share`] stop applying to it:
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
    ///     User => "users" {
    ///         UserId => "id": i64,
    ///     }
    /// }
    ///
    /// fn main() {
    ///     // Fails to compile: a deduplicated statement cannot be locked.
    ///     let _ = SelectQuery::<MockDialect>::new()
    ///         .from::<User>()
    ///         .select((UserId,))
    ///         .distinct()
    ///         .for_update();
    /// }
    /// ```
    pub fn distinct(mut self) -> SelectQuery<D, Tables, Output, Projected, LockUnavailable> {
        self.distinct = true;
        self.lock_unavailable()
    }
}

impl<D: Dialect, Tables, Output, Projected> SelectQuery<D, Tables, Output, Projected, NoLock> {
    /// Appends a column to the `DISTINCT ON (...)` clause.
    ///
    /// `DISTINCT ON` is a dialect-specific feature supported by PostgreSQL and
    /// gated by [`SupportsDistinctOn`]. The expressions of the clause have to
    /// match the leading expressions of the `ORDER BY` clause, because that
    /// ordering decides which of the duplicate rows survives; PostgreSQL rejects
    /// the statement otherwise. The rule compares the assembled expressions
    /// rather than their types, so it is checked during `.build()` and surfaces
    /// as [`BuildError::DistinctOnOrderMismatch`](crate::query::BuildError::DistinctOnOrderMismatch).
    ///
    /// A deduplicated statement cannot request a row-level lock: PostgreSQL
    /// refuses `FOR UPDATE` and `FOR SHARE` on a statement that keeps one row per
    /// group, because the lock claims the rows the statement read while the
    /// deduplication leaves one of them. The method is therefore reachable only
    /// from [`NoLock`], and it moves the query into [`LockUnavailable`], where
    /// [`SelectQuery::for_update`] and [`SelectQuery::for_share`] stop applying to
    /// it — a query that already locked its rows cannot start deduplicating them
    /// either. The pair is additionally rejected by `.build()` as
    /// [`BuildError::DistinctOnWithLock`](crate::query::BuildError::DistinctOnWithLock),
    /// so a statement assembled from AST data cannot reach the engine as well.
    ///
    /// ```compile_fail
    /// use rurqe_core::dialect::{Dialect, SupportsDistinctOn, SupportsRowLocking};
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
    /// impl SupportsDistinctOn for MockDialect {}
    ///
    /// impl SupportsRowLocking for MockDialect {}
    ///
    /// rurqe_core::table! {
    ///     User => "users" {
    ///         UserId => "id": i64,
    ///     }
    /// }
    ///
    /// fn main() {
    ///     // Fails to compile: a statement that already locked its rows cannot
    ///     // start deduplicating them, because PostgreSQL refuses the pair.
    ///     let _ = SelectQuery::<MockDialect>::new()
    ///         .from::<User>()
    ///         .select((UserId,))
    ///         .for_update()
    ///         .distinct_on(UserId);
    /// }
    /// ```
    pub fn distinct_on<C: Column, P>(
        mut self,
        _col: C,
    ) -> SelectQuery<D, Tables, Output, Projected, LockUnavailable>
    where
        D: SupportsDistinctOn,
        Tables: TableInScope<C::Table, P>,
    {
        self.distinct_on.push(C::expr_kind());
        self.lock_unavailable()
    }
}

impl<D: Dialect + SupportsDistinctOn, Tables, Output, Projected>
    SelectQuery<D, Tables, Output, Projected, LockUnavailable>
{
    /// Appends another expression to the `DISTINCT ON (...)` clause.
    ///
    /// A statement deduplicates its rows on every expression of the clause — one
    /// row per `(user_id, tenant_id)` pair, for instance — so the criteria
    /// accumulate. The first criterion moves the query into [`LockUnavailable`],
    /// and recording a further one keeps it there: the statement stays
    /// deduplicated, which is exactly what keeps [`SelectQuery::for_update`] and
    /// [`SelectQuery::for_share`] out of reach. PostgreSQL refuses
    /// `SELECT DISTINCT ON (...) ... FOR UPDATE`, because the lock claims the rows
    /// the statement read while the deduplication keeps one row per group, and a
    /// statement assembled from AST data is rejected during `.build()` as
    /// [`BuildError::DistinctOnWithLock`](crate::query::BuildError::DistinctOnWithLock)
    /// as well.
    ///
    /// ```compile_fail
    /// use rurqe_core::dialect::{Dialect, SupportsDistinctOn, SupportsRowLocking};
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
    /// impl SupportsDistinctOn for MockDialect {}
    ///
    /// impl SupportsRowLocking for MockDialect {}
    ///
    /// rurqe_core::table! {
    ///     User => "users" {
    ///         UserId => "id": i64,
    ///         UserTenant => "tenant_id": i64,
    ///     }
    /// }
    ///
    /// fn main() {
    ///     // Fails to compile: extending the DISTINCT ON clause keeps the
    ///     // statement deduplicated, so it cannot start locking its rows.
    ///     let _ = SelectQuery::<MockDialect>::new()
    ///         .from::<User>()
    ///         .select((UserId,))
    ///         .distinct_on(UserId)
    ///         .distinct_on(UserTenant)
    ///         .for_update();
    /// }
    /// ```
    pub fn distinct_on<C: Column, P>(mut self, _col: C) -> Self
    where
        Tables: TableInScope<C::Table, P>,
    {
        self.distinct_on.push(C::expr_kind());
        self
    }
}

impl<D: Dialect, Tables, Output, Projected, Lock: ProjectionOpen>
    SelectQuery<D, Tables, Output, Projected, Lock>
{
    /// Selects `selection`, fixing the row type of the query to the selection row.
    ///
    /// The selection is captured as dialect-neutral AST nodes at the call site,
    /// so the query keeps its projection as plain data and renders it with the
    /// dialect of the statement, exactly like the filter, the joins, and the
    /// ordering it already carries. Recording a projection moves the query into
    /// [`HasProjection`], the only state where it can be rendered.
    ///
    /// The query must already select from a table, so the row type is derived
    /// from the scope the projection is read from: a column of a `LEFT JOIN`ed
    /// table reaches the row as `Option<T>`.
    ///
    /// A statement records its projection once. After `UNION`, `INTERSECT`, or
    /// `EXCEPT` the output columns are the ones the first projection list named —
    /// the operands were combined on that understanding — so the query carries
    /// [`SetOpApplied`], [`ProjectionOpen`] stays unimplemented for it, and the
    /// call is rejected instead of rebinding the left operand behind the back of
    /// the operands already combined:
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
    ///     // Fails to compile: the set operation already fixed the rows the
    ///     // combined statement produces, so `select` cannot rebind them.
    ///     let _ = base().union(base()).select((UserId,));
    /// }
    /// ```
    pub fn select<S, Positions>(
        mut self,
        selection: S,
    ) -> SelectQuery<
        D,
        Tables,
        <S as SelectionFor<Tables, Positions, D>>::Row,
        HasProjection,
        Lock,
    >
    where
        Tables: TableScope,
        S: SelectionFor<Tables, Positions, D>,
    {
        let mut projections = Vec::new();
        Selection::push_projections(&selection, &mut projections);
        self.projections = projections;
        self.with_projection()
    }
}

impl<D: Dialect, Tables, Output, Projected, Lock> SelectQuery<D, Tables, Output, Projected, Lock> {
    /// Adds a Common Table Expression (`WITH cte AS (...)`) to this query.
    ///
    /// The expression was built for the dialect `CteD`, which must be the
    /// dialect of this statement or dialect-neutral, so a CTE of another engine
    /// does not compile into the query.
    pub fn with<T, Row, CteD>(mut self, cte: Cte<T, Row, CteD>) -> Self
    where
        CteD: DialectInScope<D>,
    {
        self.ctes.push(CteAst {
            name: cte.name,
            query: cte.query,
        });
        self
    }

    /// Lifts the column `C` into its nullable form [`Nullable`].
    pub fn nullable<C: Column>(&self) -> Nullable<C> {
        Nullable::new()
    }

    /// Restricts the query to rows matching `predicate`.
    pub fn where_<Scope, P, PredD>(
        mut self,
        predicate: Expr<bool, Scope, NonAggregate, PredD>,
    ) -> Self
    where
        Scope: ExprInScope<Tables, P> + ScopePositions<P>,
        PredD: DialectInScope<D>,
    {
        self.push_filter(predicate.into_kind());
        self
    }

    /// Restricts the query to rows matching `predicate` when one is given.
    ///
    /// A `SELECT` records its filter as statement data rather than as a typestate,
    /// so an absent predicate leaves the query unchanged and the returned query
    /// type stays the same.
    pub fn where_opt<Scope, P, PredD>(
        self,
        predicate: Option<Expr<bool, Scope, NonAggregate, PredD>>,
    ) -> Self
    where
        Scope: ExprInScope<Tables, P> + ScopePositions<P>,
        PredD: DialectInScope<D>,
    {
        match predicate {
            Some(predicate) => self.where_(predicate),
            None => self,
        }
    }

    /// Applies `build` to the query when `condition` holds.
    ///
    /// The closure returns the query it receives, so it restricts, orders, or
    /// paginates the statement, while a clause that changes the query type — a
    /// projection, a join, or a row lock — stays outside the conditional.
    pub fn when(self, condition: bool, build: impl FnOnce(Self) -> Self) -> Self {
        if condition {
            build(self)
        } else {
            self
        }
    }

    /// Applies `build` to the query when `value` is present, handing it over.
    pub fn if_some<T>(self, value: Option<T>, build: impl FnOnce(Self, T) -> Self) -> Self {
        match value {
            Some(value) => build(self, value),
            None => self,
        }
    }

    /// Merges `predicate` into the recorded filter with a boolean `AND`.
    fn push_filter(&mut self, predicate: ExprKind) {
        let filter = match self.filter.take() {
            Some(existing) => ExprKind::Binary {
                op: BinaryOp::And,
                lhs: Box::new(existing),
                rhs: Box::new(predicate),
            },
            None => predicate,
        };
        self.filter = Some(filter);
    }

    /// Records `predicate` as the `HAVING` clause of the statement.
    ///
    /// A `HAVING` clause filters the groups an aggregate produced, so the
    /// predicate is written over an aggregate — `count_all().gt(5i64)`, for
    /// instance — and keeps the aggregation marker of that comparison.
    ///
    /// Only a grouped statement has groups to filter, so the clause is recorded
    /// through [`GroupedQuery::having`](crate::query::GroupedQuery::having) and
    /// [`GroupedQuery::having_col`](crate::query::GroupedQuery::having_col): the
    /// rules that make a predicate valid read the grouping the query recorded, and
    /// that grouping is known only while the query carries the grouped typestate.
    /// The method therefore stays crate-private, which keeps an ungrouped query
    /// from rendering a `HAVING` clause the engine would reject.
    pub(crate) fn having<Scope, Aggr, P, PredD>(
        mut self,
        predicate: Expr<bool, Scope, Aggr, PredD>,
    ) -> Self
    where
        Scope: ExprInScope<Tables, P> + ScopePositions<P>,
        PredD: DialectInScope<D>,
        Aggr: AggregateInDialect<D>,
    {
        let predicate = predicate.into_kind();
        let having = match self.having.take() {
            Some(existing) => ExprKind::Binary {
                op: BinaryOp::And,
                lhs: Box::new(existing),
                rhs: Box::new(predicate),
            },
            None => predicate,
        };
        self.having = Some(having);
        self
    }
}
