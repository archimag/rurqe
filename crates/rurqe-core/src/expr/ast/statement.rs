//! Dialect-neutral statement AST nodes and the dialect markers guarding them.
//!
//! The nodes describe a statement as plain data: they carry neither rendering
//! closures nor a dialect parameter, so two statements built through different
//! builders compare equal when they describe the same SQL, and the dialect is
//! supplied only when the statement is rendered. The enumerations describing the
//! statement shape live here as well, so the expression AST and the query builder
//! share one definition of every node.

use core::fmt::Write;

use crate::dialect::Dialect;
use crate::schema::TableRef;

use super::ExprKind;

/// The direction of an `ORDER BY` clause.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OrderDirection {
    /// Ascending order (`ASC`).
    Asc,
    /// Descending order (`DESC`).
    Desc,
}

impl OrderDirection {
    /// The SQL text of the direction.
    pub const fn as_str(self) -> &'static str {
        match self {
            OrderDirection::Asc => "ASC",
            OrderDirection::Desc => "DESC",
        }
    }
}

/// The flavour of join bringing a relation into the statement scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum JoinKind {
    /// `INNER JOIN`, which only keeps rows matched in the joined relation.
    Inner,
    /// `LEFT JOIN`, which keeps unmatched rows and nulls the joined columns.
    Left,
    /// `CROSS JOIN LATERAL`, which joins every outer row with a correlated subquery.
    CrossLateral,
    /// `LEFT JOIN LATERAL ... ON true`, which keeps outer rows a lateral subquery
    /// leaves unmatched.
    LeftLateral,
}

impl JoinKind {
    /// The SQL text of the join keywords.
    pub const fn as_str(self) -> &'static str {
        match self {
            JoinKind::Inner => "INNER JOIN",
            JoinKind::Left => "LEFT JOIN",
            JoinKind::CrossLateral => "CROSS JOIN LATERAL",
            JoinKind::LeftLateral => "LEFT JOIN LATERAL",
        }
    }
}

/// The flavour of a relational set operation combining two statements.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SetOpKind {
    /// `UNION` combines the rows of both statements and removes duplicates.
    Union,
    /// `UNION ALL` combines the rows of both statements and keeps duplicates.
    UnionAll,
    /// `INTERSECT` keeps only the rows produced by both statements.
    Intersect,
    /// `EXCEPT` keeps only the rows of the left statement the right one lacks.
    Except,
}

impl SetOpKind {
    /// The SQL text of the set operation keywords.
    pub const fn as_str(self) -> &'static str {
        match self {
            SetOpKind::Union => "UNION",
            SetOpKind::UnionAll => "UNION ALL",
            SetOpKind::Intersect => "INTERSECT",
            SetOpKind::Except => "EXCEPT",
        }
    }
}

/// The strength of the row-level lock a statement requests over its rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LockStrength {
    /// `FOR UPDATE`, an exclusive lock on the selected rows.
    Update,
    /// `FOR SHARE`, a shared lock leaving readers concurrent.
    Share,
}

impl LockStrength {
    /// The SQL text of the lock clause.
    pub const fn as_str(self) -> &'static str {
        match self {
            LockStrength::Update => "FOR UPDATE",
            LockStrength::Share => "FOR SHARE",
        }
    }
}

/// How a statement behaves when a selected row is already locked elsewhere.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LockWait {
    /// Wait for the concurrent transaction to finish before locking the row.
    Block,
    /// Fail immediately instead of waiting (`NOWAIT`).
    NoWait,
    /// Skip the locked rows and lock the rest (`SKIP LOCKED`).
    SkipLocked,
}

impl LockWait {
    /// The SQL text appended to the lock clause, prefixed by a space.
    pub const fn as_str(self) -> &'static str {
        match self {
            LockWait::Block => "",
            LockWait::NoWait => " NOWAIT",
            LockWait::SkipLocked => " SKIP LOCKED",
        }
    }
}

/// The row-level lock a statement requests over the rows it selects.
///
/// The clause closes the statement, so a locked query reads as
/// `SELECT ... LIMIT 1 FOR UPDATE SKIP LOCKED`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LockClause {
    strength: LockStrength,
    wait: LockWait,
}

impl LockClause {
    /// Requests `strength` while waiting for rows locked elsewhere.
    pub const fn new(strength: LockStrength) -> Self {
        Self {
            strength,
            wait: LockWait::Block,
        }
    }

    /// The strength of the requested lock.
    pub const fn strength(self) -> LockStrength {
        self.strength
    }

    /// The wait behaviour of the requested lock.
    pub const fn wait(self) -> LockWait {
        self.wait
    }

    /// Fails instead of waiting for rows locked elsewhere (`NOWAIT`).
    pub const fn nowait(self) -> Self {
        Self {
            strength: self.strength,
            wait: LockWait::NoWait,
        }
    }

    /// Skips rows locked elsewhere and locks the rest (`SKIP LOCKED`).
    pub const fn skip_locked(self) -> Self {
        Self {
            strength: self.strength,
            wait: LockWait::SkipLocked,
        }
    }

    /// Appends the clause to `sql`, prefixed by a space.
    pub fn render_into(self, sql: &mut String) {
        sql.push(' ');
        sql.push_str(self.strength.as_str());
        sql.push_str(self.wait.as_str());
    }
}

/// The flavour of a window frame restricting the rows a window function reads.
///
/// `ROWS` counts the offset of a boundary in physical rows, while `RANGE`
/// measures it in values of the ordered column, so the same boundary renders
/// under either mode but selects a different set of rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WindowFrameKind {
    /// `ROWS`, counting boundaries in physical rows.
    Rows,
    /// `RANGE`, measuring boundaries in values of the ordered column.
    Range,
}

impl WindowFrameKind {
    /// The SQL keyword of the frame mode.
    pub const fn as_str(self) -> &'static str {
        match self {
            WindowFrameKind::Rows => "ROWS",
            WindowFrameKind::Range => "RANGE",
        }
    }
}

/// One boundary of a window frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WindowFrameBound {
    /// The frame reaches back to the start of the partition (`UNBOUNDED PRECEDING`).
    UnboundedPreceding,
    /// The boundary lies `offset` rows (or range units) before the current row.
    Preceding(u32),
    /// The boundary is the current row itself (`CURRENT ROW`).
    CurrentRow,
    /// The boundary lies `offset` rows (or range units) after the current row.
    Following(u32),
    /// The frame reaches forward to the end of the partition (`UNBOUNDED FOLLOWING`).
    UnboundedFollowing,
}

impl WindowFrameBound {
    /// Builds a boundary `offset` rows (or range units) before the current row.
    pub const fn preceding(offset: u32) -> Self {
        WindowFrameBound::Preceding(offset)
    }

    /// Builds a boundary `offset` rows (or range units) after the current row.
    pub const fn following(offset: u32) -> Self {
        WindowFrameBound::Following(offset)
    }

    /// Appends the boundary to `out`, prefixed by nothing.
    pub fn render_into(self, out: &mut String) {
        match self {
            WindowFrameBound::UnboundedPreceding => out.push_str("UNBOUNDED PRECEDING"),
            WindowFrameBound::Preceding(offset) => {
                let _ = write!(out, "{offset} PRECEDING");
            }
            WindowFrameBound::CurrentRow => out.push_str("CURRENT ROW"),
            WindowFrameBound::Following(offset) => {
                let _ = write!(out, "{offset} FOLLOWING");
            }
            WindowFrameBound::UnboundedFollowing => out.push_str("UNBOUNDED FOLLOWING"),
        }
    }
}

/// The `ROWS` or `RANGE` frame restricting the rows a window function reads.
///
/// The frame closes the contents of an `OVER (...)` clause, so a sliding window
/// reads as `ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW` after the
/// partition and ordering of the specification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WindowFrame {
    kind: WindowFrameKind,
    start: WindowFrameBound,
    end: WindowFrameBound,
}

impl WindowFrame {
    /// Creates a frame of `kind` spanning `start` through `end`.
    pub const fn new(
        kind: WindowFrameKind,
        start: WindowFrameBound,
        end: WindowFrameBound,
    ) -> Self {
        Self { kind, start, end }
    }

    /// The mode of the frame.
    pub const fn kind(self) -> WindowFrameKind {
        self.kind
    }

    /// The boundary the frame starts at.
    pub const fn start(self) -> WindowFrameBound {
        self.start
    }

    /// The boundary the frame ends at.
    pub const fn end(self) -> WindowFrameBound {
        self.end
    }

    /// Appends the frame clause, such as `ROWS BETWEEN ... AND ...`, to `out`.
    pub fn render_into(self, out: &mut String) {
        out.push_str(self.kind.as_str());
        out.push_str(" BETWEEN ");
        self.start.render_into(out);
        out.push_str(" AND ");
        self.end.render_into(out);
    }
}

/// The shape of a window function call evaluated over an `OVER (...)` clause.
#[derive(Debug, Clone, PartialEq)]
pub enum WindowExprKind {
    /// `ROW_NUMBER()`
    RowNumber,
    /// `RANK()`
    Rank,
    /// `DENSE_RANK()`
    DenseRank,
    /// `LAG(column [, offset])`
    Lag {
        /// The column expression read from a preceding row.
        column: ExprKind,
        /// The optional row offset, defaulting to the previous row.
        offset: Option<u32>,
    },
    /// `LEAD(column [, offset])`
    Lead {
        /// The column expression read from a following row.
        column: ExprKind,
        /// The optional row offset, defaulting to the next row.
        offset: Option<u32>,
    },
    /// An arbitrary expression, such as an aggregate, evaluated over the window.
    Expr(ExprKind),
}

/// A `SELECT` statement described as plain, dialect-neutral data.
///
/// The statement owns its `WITH`, projection, join, filter, and grouping clauses,
/// together with the ordering and pagination that belong to the left query. A
/// statement chained with set operations keeps the ordering and pagination of the
/// combined rows separate in
/// [`compound_order_by`](SelectAst::compound_order_by),
/// [`compound_limit`](SelectAst::compound_limit), and
/// [`compound_offset`](SelectAst::compound_offset), because SQL binds those
/// clauses to the compound result rather than to the left query.
#[derive(Debug, Clone, PartialEq)]
pub struct SelectAst {
    /// The Common Table Expressions of the statement, in declaration order.
    pub ctes: Vec<CteAst>,
    /// Whether the projected rows are deduplicated (`DISTINCT`).
    pub distinct: bool,
    /// The expressions of the `DISTINCT ON (...)` clause.
    pub distinct_on: Vec<ExprKind>,
    /// The projected expressions, in selection order.
    pub projections: Vec<ExprKind>,
    /// The relation rows are selected from, rendered into the `FROM` clause.
    pub from: Option<TableRef>,
    /// The joins appended to the `FROM` clause, in declaration order.
    pub joins: Vec<JoinAst>,
    /// The `WHERE` predicate restricting the selected rows.
    pub filter: Option<ExprKind>,
    /// The expressions of the `GROUP BY` clause.
    pub group_by: Vec<ExprKind>,
    /// The `HAVING` predicate restricting the grouped rows.
    pub having: Option<ExprKind>,
    /// The ordering criteria of the left query, applied before a set operation.
    pub order_by: Vec<(ExprKind, OrderDirection)>,
    /// The row limit of the left query, applied before a set operation.
    pub limit: Option<u32>,
    /// The row offset of the left query, applied before a set operation.
    pub offset: Option<u32>,
    /// The set operations chained onto the statement, in declaration order.
    pub set_operations: Vec<SetOpAst>,
    /// The ordering criteria of the compound result, applied after a set operation.
    pub compound_order_by: Vec<(ExprKind, OrderDirection)>,
    /// The row limit of the compound result, applied after a set operation.
    pub compound_limit: Option<u32>,
    /// The row offset of the compound result, applied after a set operation.
    pub compound_offset: Option<u32>,
    /// The row-level lock requested over the selected rows, if any.
    pub lock: Option<LockClause>,
}

/// A Common Table Expression (`WITH name AS (...)`) contributing to a statement.
#[derive(Debug, Clone, PartialEq)]
pub struct CteAst {
    /// The SQL identifier the expression is named after.
    pub name: &'static str,
    /// The statement the expression is defined by.
    pub query: SelectAst,
}

/// A join bringing a relation into the scope of a [`SelectAst`].
#[derive(Debug, Clone, PartialEq)]
pub struct JoinAst {
    /// The flavour of the join.
    pub kind: JoinKind,
    /// The joined relation and its optional alias.
    pub table: TableRef,
    /// The lateral subquery joined instead of a plain relation.
    pub lateral: Option<SelectAst>,
    /// The `ON` predicate restricting the joined rows.
    pub on: Option<ExprKind>,
}

/// A set operation combining a [`SelectAst`] with another statement.
#[derive(Debug, Clone, PartialEq)]
pub struct SetOpAst {
    /// The flavour of the set operation.
    pub kind: SetOpKind,
    /// The statement combined with the one the operation is chained onto.
    pub query: SelectAst,
}

/// Statically asserts that an AST node built for the dialect `Self` can be
/// rendered by the dialect `D`.
///
/// A node that depends on no dialect carries `()` and renders under every
/// dialect, while a node pinned to a dialect renders only under that same
/// dialect.
#[diagnostic::on_unimplemented(
    message = "dialect `{D}` cannot render an AST node built for dialect `{Self}`",
    label = "the node was built for a different dialect",
    note = "dialect-neutral nodes keep `()` as their marker and render under every dialect"
)]
pub trait DialectInScope<D> {}

// A dialect-neutral node renders under every dialect.
impl<D> DialectInScope<D> for () {}

// A node pinned to a dialect renders under that same dialect.
impl<D: Dialect> DialectInScope<D> for D {}

/// Combines the dialect markers `Self` and `R` into the dialect of an AST node.
///
/// A node that depends on no dialect carries `()`, which contributes nothing.
/// Two concrete dialects combine only when they are the same dialect, so mixing
/// an expression built for one engine into a statement of another engine simply
/// does not compile.
#[diagnostic::on_unimplemented(
    message = "the dialect markers `{Self}` and `{R}` cannot be combined",
    label = "unsupported dialect combination",
    note = "dialect markers are `()` for a dialect-neutral node or a `Dialect` type such as `Postgres<Pg16>`"
)]
pub trait MergeDialect<R> {
    /// The dialect marker referencing both `Self` and `R`.
    type Output;
}

/// The dialect marker produced by combining `L` and `R`.
pub type MergedDialect<L, R> = <L as MergeDialect<R>>::Output;

// A dialect-neutral right operand adds nothing to the merged marker.
impl<L> MergeDialect<()> for L {
    type Output = L;
}

// A dialect-neutral left operand yields the dialect of its right operand.
impl<R: Dialect> MergeDialect<R> for () {
    type Output = R;
}

// Two concrete markers combine only when they name the same dialect.
impl<D: Dialect> MergeDialect<D> for D {
    type Output = D;
}
