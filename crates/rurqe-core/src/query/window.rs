//! Window function builders, `OVER` specifications, and window expressions.
//!
//! Every column a window specification references is carried as an
//! [`ExprKind`] instead of a bare identifier, so a column addressed through a
//! table alias keeps its `"alias"."column"` reference inside `PARTITION BY`,
//! `ORDER BY`, `LAG`, and `LEAD` just like it does in the projection list.
//!
//! Beside the rendered clauses, a specification and the window expressions built
//! over it carry the tables their columns belong to. A window reading a table the
//! query does not select from is therefore rejected at the call site instead of
//! rendering a statement the database cannot resolve.

use core::fmt::{self, Write};
use core::marker::PhantomData;

use crate::dialect::Dialect;
use crate::expr::{render_expr, ExprKind, MergeScope, Merged, SqlVal};
use crate::schema::{Column, ExprInScope, TableRef};

use super::scope::ScopePositions;
use super::select::OrderDirection;
use super::selection::{SelectTarget, SelectTargetInScope};

/// A column or tuple of columns for the `PARTITION BY` clause of a window specification.
#[diagnostic::on_unimplemented(
    message = "`{Self}` is not a valid PARTITION BY clause",
    label = "partition by a column or a tuple of columns",
    note = "partition by accepts column markers or tuples of column markers, such as `UserId` or `(UserId, UserName)`"
)]
pub trait PartitionBy {
    /// The tables the partitioned columns belong to.
    type Scope;

    /// Appends the partition column expressions to `out`.
    fn push_partition_by(&self, out: &mut Vec<ExprKind>);
}

impl<C: Column> PartitionBy for C {
    type Scope = C::Table;

    fn push_partition_by(&self, out: &mut Vec<ExprKind>) {
        out.push(C::expr_kind());
    }
}

macro_rules! impl_partition_by {
    ($($column:ident),+) => {
        impl<$($column: Column),+> PartitionBy for ($($column,)+) {
            type Scope = ($($column::Table,)+);

            fn push_partition_by(&self, out: &mut Vec<ExprKind>) {
                $(out.push($column::expr_kind());)+
            }
        }
    };
}

impl_partition_by!(C1);
impl_partition_by!(C1, C2);
impl_partition_by!(C1, C2, C3);
impl_partition_by!(C1, C2, C3, C4);
impl_partition_by!(C1, C2, C3, C4, C5);
impl_partition_by!(C1, C2, C3, C4, C5, C6);
impl_partition_by!(C1, C2, C3, C4, C5, C6, C7);
impl_partition_by!(C1, C2, C3, C4, C5, C6, C7, C8);

/// A specification of an `OVER (...)` window partition and ordering.
///
/// The specification carries the tables `Scope` its partitioned and ordered
/// columns belong to, so a window function evaluated over it is only projectable
/// from a query selecting from those tables.
pub struct WindowSpec<Scope = ()> {
    partition_by: Vec<ExprKind>,
    order_by: Vec<(ExprKind, OrderDirection)>,
    _scope: PhantomData<fn() -> Scope>,
}

impl<Scope> Default for WindowSpec<Scope> {
    fn default() -> Self {
        Self {
            partition_by: Vec::new(),
            order_by: Vec::new(),
            _scope: PhantomData,
        }
    }
}

impl<Scope> Clone for WindowSpec<Scope> {
    fn clone(&self) -> Self {
        Self {
            partition_by: self.partition_by.clone(),
            order_by: self.order_by.clone(),
            _scope: PhantomData,
        }
    }
}

impl<Scope> PartialEq for WindowSpec<Scope> {
    fn eq(&self, other: &Self) -> bool {
        self.partition_by == other.partition_by && self.order_by == other.order_by
    }
}

impl<Scope> fmt::Debug for WindowSpec<Scope> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WindowSpec")
            .field("partition_by", &self.partition_by)
            .field("order_by", &self.order_by)
            .finish()
    }
}

impl<Scope> WindowSpec<Scope> {
    /// Creates an empty window specification (`OVER ()`).
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates an empty window specification (`OVER ()`).
    pub fn empty() -> Self {
        Self::default()
    }

    /// Partitions the window by `partition`, extending the scope by its tables.
    pub fn partition_by<P>(self, partition: P) -> WindowSpec<Merged<Scope, P::Scope>>
    where
        P: PartitionBy,
        Scope: MergeScope<P::Scope>,
    {
        let mut partition_by = self.partition_by;
        partition.push_partition_by(&mut partition_by);
        WindowSpec {
            partition_by,
            order_by: self.order_by,
            _scope: PhantomData,
        }
    }

    /// Appends an ordering criterion with `direction` to the window.
    pub fn order_by<C: Column>(
        mut self,
        _col: C,
        direction: OrderDirection,
    ) -> WindowSpec<Merged<Scope, C::Table>>
    where
        Scope: MergeScope<C::Table>,
    {
        self.order_by.push((C::expr_kind(), direction));
        self.retype()
    }

    /// Appends an ascending ordering criterion for column `col`.
    pub fn order_by_asc<C: Column>(mut self, _col: C) -> WindowSpec<Merged<Scope, C::Table>>
    where
        Scope: MergeScope<C::Table>,
    {
        self.order_by.push((C::expr_kind(), OrderDirection::Asc));
        self.retype()
    }

    /// Appends a descending ordering criterion for column `col`.
    pub fn order_by_desc<C: Column>(mut self, _col: C) -> WindowSpec<Merged<Scope, C::Table>>
    where
        Scope: MergeScope<C::Table>,
    {
        self.order_by.push((C::expr_kind(), OrderDirection::Desc));
        self.retype()
    }

    /// Renders the window contents inside `OVER (...)`, qualifying the
    /// partitioned and ordered columns against `relations`.
    pub fn render<D: Dialect>(
        &self,
        sql: &mut String,
        params: &mut Vec<SqlVal>,
        relations: &[TableRef],
    ) {
        let mut has_part = false;
        if !self.partition_by.is_empty() {
            has_part = true;
            sql.push_str("PARTITION BY ");
            for (i, expression) in self.partition_by.iter().enumerate() {
                if i > 0 {
                    sql.push_str(", ");
                }
                render_expr::<D>(sql, params, expression, relations);
            }
        }
        if !self.order_by.is_empty() {
            if has_part {
                sql.push(' ');
            }
            sql.push_str("ORDER BY ");
            for (i, (expression, dir)) in self.order_by.iter().enumerate() {
                if i > 0 {
                    sql.push_str(", ");
                }
                render_expr::<D>(sql, params, expression, relations);
                sql.push(' ');
                sql.push_str(dir.as_str());
            }
        }
    }

    /// Re-types the specification to `NewScope` while keeping the rendered clauses.
    ///
    /// The builders extend the clauses in place and only change the scope marker,
    /// so the transition between scopes is expressed exactly once.
    fn retype<NewScope>(self) -> WindowSpec<NewScope> {
        WindowSpec {
            partition_by: self.partition_by,
            order_by: self.order_by,
            _scope: PhantomData,
        }
    }

    /// Drops the scope marker while keeping the rendered clauses.
    ///
    /// A window expression carries the union of its own scope and the scope of
    /// its specification, so the stored specification does not need to repeat it.
    fn without_scope(self) -> WindowSpec<()> {
        self.retype()
    }
}

/// Entry point for constructing window specifications.
pub struct Window;

impl Window {
    /// Creates a window specification referencing no table.
    #[allow(clippy::new_ret_no_self)]
    pub fn new() -> WindowSpec<()> {
        WindowSpec::default()
    }

    /// Creates a window specification referencing no table.
    pub fn empty() -> WindowSpec<()> {
        WindowSpec::default()
    }

    /// Starts a window specification partitioned by `partition`.
    pub fn partition_by<P: PartitionBy>(partition: P) -> WindowSpec<P::Scope> {
        let mut partition_by = Vec::new();
        partition.push_partition_by(&mut partition_by);
        WindowSpec {
            partition_by,
            order_by: Vec::new(),
            _scope: PhantomData,
        }
    }

    /// Starts a window specification ordered by `col` with `direction`.
    pub fn order_by<C: Column>(_col: C, direction: OrderDirection) -> WindowSpec<C::Table> {
        WindowSpec {
            partition_by: Vec::new(),
            order_by: vec![(C::expr_kind(), direction)],
            _scope: PhantomData,
        }
    }

    /// Starts a window specification with ascending ordering for `col`.
    pub fn order_by_asc<C: Column>(col: C) -> WindowSpec<C::Table> {
        Window::order_by(col, OrderDirection::Asc)
    }

    /// Starts a window specification with descending ordering for `col`.
    pub fn order_by_desc<C: Column>(col: C) -> WindowSpec<C::Table> {
        Window::order_by(col, OrderDirection::Desc)
    }
}

/// The internal representation of a window function call.
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
        /// The target column expression.
        column: ExprKind,
        /// The optional row offset.
        offset: Option<u32>,
    },
    /// `LEAD(column [, offset])`
    Lead {
        /// The target column expression.
        column: ExprKind,
        /// The optional row offset.
        offset: Option<u32>,
    },
    /// An arbitrary expression (such as an aggregate) evaluated over the window.
    Expr(ExprKind),
}

/// A typed window expression evaluated with an `OVER (...)` clause.
///
/// The expression carries the tables `Scope` referenced by the wrapped
/// expression and by its window specification.
pub struct WindowExpr<T, Scope = ()> {
    kind: WindowExprKind,
    window: WindowSpec<()>,
    _result: PhantomData<fn() -> (T, Scope)>,
}

impl<T, Scope> Clone for WindowExpr<T, Scope> {
    fn clone(&self) -> Self {
        Self {
            kind: self.kind.clone(),
            window: self.window.clone(),
            _result: PhantomData,
        }
    }
}

impl<T, Scope> PartialEq for WindowExpr<T, Scope> {
    fn eq(&self, other: &Self) -> bool {
        self.kind == other.kind && self.window == other.window
    }
}

impl<T, Scope> fmt::Debug for WindowExpr<T, Scope> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WindowExpr")
            .field("kind", &self.kind)
            .field("window", &self.window)
            .finish()
    }
}

impl<T, Scope> WindowExpr<T, Scope> {
    /// Creates a window expression of scope `Scope` over `window`.
    ///
    /// The caller is responsible for `Scope` covering both the wrapped
    /// expression and the window specification, which is exactly what the
    /// `over` builders of the expressions and of `LAG` / `LEAD` arrange for.
    pub(crate) fn of_window<WinScope>(kind: WindowExprKind, window: WindowSpec<WinScope>) -> Self {
        Self {
            kind,
            window: window.without_scope(),
            _result: PhantomData,
        }
    }

    /// Renders this window function and its `OVER (...)` clause to `sql`.
    ///
    /// `relations` names the relations of the enclosing statement and qualifies
    /// every column the window reads or orders by.
    pub fn render<D: Dialect>(
        &self,
        sql: &mut String,
        params: &mut Vec<SqlVal>,
        relations: &[TableRef],
    ) {
        match &self.kind {
            WindowExprKind::RowNumber => sql.push_str("ROW_NUMBER()"),
            WindowExprKind::Rank => sql.push_str("RANK()"),
            WindowExprKind::DenseRank => sql.push_str("DENSE_RANK()"),
            WindowExprKind::Lag { column, offset } => {
                sql.push_str("LAG(");
                render_expr::<D>(sql, params, column, relations);
                if let Some(off) = offset {
                    let _ = write!(sql, ", {off}");
                }
                sql.push(')');
            }
            WindowExprKind::Lead { column, offset } => {
                sql.push_str("LEAD(");
                render_expr::<D>(sql, params, column, relations);
                if let Some(off) = offset {
                    let _ = write!(sql, ", {off}");
                }
                sql.push(')');
            }
            WindowExprKind::Expr(expr) => {
                render_expr::<D>(sql, params, expr, relations);
            }
        }
        sql.push_str(" OVER (");
        self.window.render::<D>(sql, params, relations);
        sql.push(')');
    }
}

impl<T> WindowExpr<T, ()> {
    /// Creates a window expression wrapping the expression `kind` over `window`.
    ///
    /// The window must reference no table, so a specification partitioned or
    /// ordered by a column is wrapped through the `over` builders instead.
    pub fn from_expr(kind: ExprKind, window: WindowSpec<()>) -> Self {
        Self::of_window(WindowExprKind::Expr(kind), window)
    }
}

impl<T: 'static + Send + Sync, Scope: 'static + Send + Sync> SelectTarget
    for WindowExpr<T, Scope>
{
    type RustType = T;

    fn render<D: Dialect>(&self, sql: &mut String, params: &mut Vec<SqlVal>, relations: &[TableRef]) {
        WindowExpr::render::<D>(self, sql, params, relations);
    }
}

impl<Tables, T, Scope, P> SelectTargetInScope<Tables, P> for WindowExpr<T, Scope>
where
    Scope: ExprInScope<Tables, P> + ScopePositions<P>,
{
    type Output = T;
}

/// A window function awaiting an `OVER (...)` specification.
#[derive(Debug, Clone, PartialEq)]
pub struct WindowFn<T> {
    kind: WindowExprKind,
    _result: PhantomData<fn() -> T>,
}

impl<T> WindowFn<T> {
    /// Creates a new window function builder.
    pub fn new(kind: WindowExprKind) -> Self {
        Self {
            kind,
            _result: PhantomData,
        }
    }

    /// Evaluates this window function over `window`.
    pub fn over<WinScope>(self, window: WindowSpec<WinScope>) -> WindowExpr<T, WinScope> {
        WindowExpr::of_window(self.kind, window)
    }
}

/// A `LAG` or `LEAD` window function builder with optional offset configuration.
///
/// The builder carries the table of the column it reads as `Scope`, which the
/// resulting window expression merges with the scope of its specification.
#[derive(Debug, Clone, PartialEq)]
pub struct LagLeadFn<T, Scope = ()> {
    is_lag: bool,
    column: ExprKind,
    offset: Option<u32>,
    _result: PhantomData<fn() -> (T, Scope)>,
}

impl<T, Scope> LagLeadFn<T, Scope> {
    /// Sets the row offset (defaults to 1 if omitted).
    pub fn offset(mut self, offset: u32) -> Self {
        self.offset = Some(offset);
        self
    }

    /// Evaluates this function over `window`.
    pub fn over<WinScope>(
        self,
        window: WindowSpec<WinScope>,
    ) -> WindowExpr<T, Merged<Scope, WinScope>>
    where
        Scope: MergeScope<WinScope>,
    {
        let kind = if self.is_lag {
            WindowExprKind::Lag {
                column: self.column,
                offset: self.offset,
            }
        } else {
            WindowExprKind::Lead {
                column: self.column,
                offset: self.offset,
            }
        };
        WindowExpr::of_window(kind, window)
    }
}

/// Starts a `ROW_NUMBER()` window function builder.
pub fn row_number() -> WindowFn<i64> {
    WindowFn::new(WindowExprKind::RowNumber)
}

/// Starts a `RANK()` window function builder.
pub fn rank() -> WindowFn<i64> {
    WindowFn::new(WindowExprKind::Rank)
}

/// Starts a `DENSE_RANK()` window function builder.
pub fn dense_rank() -> WindowFn<i64> {
    WindowFn::new(WindowExprKind::DenseRank)
}

/// Starts a `LAG(C)` window function builder for column `C`.
pub fn lag<C: Column>(_col: C) -> LagLeadFn<Option<C::RustType>, C::Table> {
    LagLeadFn {
        is_lag: true,
        column: C::expr_kind(),
        offset: None,
        _result: PhantomData,
    }
}

/// Starts a `LEAD(C)` window function builder for column `C`.
pub fn lead<C: Column>(_col: C) -> LagLeadFn<Option<C::RustType>, C::Table> {
    LagLeadFn {
        is_lag: false,
        column: C::expr_kind(),
        offset: None,
        _result: PhantomData,
    }
}
