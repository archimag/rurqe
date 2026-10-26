//! The type-safe `SELECT` builder and its single-pass SQL renderer.

use core::marker::PhantomData;

use crate::dialect::Dialect;
use crate::expr::{CteAst, DerivedAst, ExprKind, JoinAst, LockClause, SqlVal};
use crate::schema::TableRef;

// `ORDER BY` criteria describe the statement AST shared with the expression
// module, so the canonical definitions live there and are re-exported here.
pub use crate::expr::{NullsOrder, OrderCriterion, OrderDirection};

use super::set_ops::SetOpAst;

// The builder is decomposed by responsibility: `markers` declares the typestates a
// statement passes through, `traits` names the compile-time assertions those
// typestates back, `from` introduces the relation a statement selects from,
// `clauses` records the projections and filters written around it, joins establish
// the relational scope, ordering and pagination move rows of the clause they are
// written on, locking and set operations change the typestate of the statement, the
// transitions module names every typestate change a builder may make, and the
// rendering adapters convert the query into its statement AST.
mod clauses;
mod from;
mod joins;
mod locking;
mod markers;
mod ordering;
mod render;
mod set_ops;
mod traits;
mod transitions;

pub use self::markers::{
    HasProjection, LockUnavailable, LockWaitSet, Locked, NoLock, NoProjection, SetOpApplied,
    TailClosed,
};
pub use self::ordering::OrderBy;
pub use self::traits::{ProjectionOpen, ProjectionPresent, SetOperand, Unlocked};

/// The rendered SQL text of a query together with its bound parameters.
#[derive(Debug, Clone, PartialEq)]
pub struct BuiltQuery {
    /// The SQL text using the dialect's positional placeholders.
    pub sql: String,

    /// The parameter values referenced by the placeholders, in placeholder order.
    pub params: Vec<SqlVal>,
}

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
