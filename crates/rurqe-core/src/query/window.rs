//! Window function builders, `OVER` specifications, and window expressions.
//!
//! Every column a window specification references is carried as an
//! [`ExprKind`] instead of a bare identifier, so a column addressed through a
//! table alias keeps its `"alias"."column"` reference inside `PARTITION BY`,
//! `ORDER BY`, `LAG`, and `LEAD` just like it does in the projection list.

use core::fmt::Write;
use core::marker::PhantomData;

use crate::dialect::Dialect;
use crate::expr::{render_expr, ExprKind, SqlVal};
use crate::schema::{At, Column};

use super::select::OrderDirection;
use super::selection::{SelectTarget, SelectTargetInScope};

/// A column or tuple of columns for the `PARTITION BY` clause of a window specification.
#[diagnostic::on_unimplemented(
    message = "`{Self}` is not a valid PARTITION BY clause",
    label = "partition by a column or a tuple of columns",
    note = "partition by accepts column markers or tuples of column markers, such as `UserId` or `(UserId, UserName)`"
)]
pub trait PartitionBy {
    /// Appends the partition column expressions to `out`.
    fn push_partition_by(&self, out: &mut Vec<ExprKind>);
}

impl<C: Column> PartitionBy for C {
    fn push_partition_by(&self, out: &mut Vec<ExprKind>) {
        out.push(C::expr_kind());
    }
}

macro_rules! impl_partition_by {
    ($($column:ident),+) => {
        impl<$($column: Column),+> PartitionBy for ($($column,)+) {
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
#[derive(Debug, Clone, Default, PartialEq)]
pub struct WindowSpec {
    partition_by: Vec<ExprKind>,
    order_by: Vec<(ExprKind, OrderDirection)>,
}

impl WindowSpec {
    /// Creates an empty window specification (`OVER ()`).
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates an empty window specification (`OVER ()`).
    pub fn empty() -> Self {
        Self::default()
    }

    /// Partitions the window by `partition`.
    pub fn partition_by<P: PartitionBy>(mut self, partition: P) -> Self {
        partition.push_partition_by(&mut self.partition_by);
        self
    }

    /// Appends an ordering criterion with `direction` to the window.
    pub fn order_by<C: Column>(mut self, _col: C, direction: OrderDirection) -> Self {
        self.order_by.push((C::expr_kind(), direction));
        self
    }

    /// Appends an ascending ordering criterion for column `col`.
    pub fn order_by_asc<C: Column>(self, col: C) -> Self {
        self.order_by(col, OrderDirection::Asc)
    }

    /// Appends a descending ordering criterion for column `col`.
    pub fn order_by_desc<C: Column>(self, col: C) -> Self {
        self.order_by(col, OrderDirection::Desc)
    }

    /// Renders the window contents inside `OVER (...)`.
    pub fn render<D: Dialect>(&self, sql: &mut String, params: &mut Vec<SqlVal>) {
        let mut has_part = false;
        if !self.partition_by.is_empty() {
            has_part = true;
            sql.push_str("PARTITION BY ");
            for (i, expression) in self.partition_by.iter().enumerate() {
                if i > 0 {
                    sql.push_str(", ");
                }
                render_expr::<D>(sql, params, expression);
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
                render_expr::<D>(sql, params, expression);
                sql.push(' ');
                sql.push_str(dir.as_str());
            }
        }
    }
}

/// Entry point for constructing window specifications.
pub struct Window;

impl Window {
    /// Creates a new empty window specification.
    #[allow(clippy::new_ret_no_self)]
    pub fn new() -> WindowSpec {
        WindowSpec::new()
    }

    /// Creates a new empty window specification.
    pub fn empty() -> WindowSpec {
        WindowSpec::empty()
    }

    /// Starts a window specification partitioned by `partition`.
    pub fn partition_by<P: PartitionBy>(partition: P) -> WindowSpec {
        WindowSpec::empty().partition_by(partition)
    }

    /// Starts a window specification ordered by `col` with `direction`.
    pub fn order_by<C: Column>(col: C, direction: OrderDirection) -> WindowSpec {
        WindowSpec::empty().order_by(col, direction)
    }

    /// Starts a window specification with ascending ordering for `col`.
    pub fn order_by_asc<C: Column>(col: C) -> WindowSpec {
        WindowSpec::empty().order_by_asc(col)
    }

    /// Starts a window specification with descending ordering for `col`.
    pub fn order_by_desc<C: Column>(col: C) -> WindowSpec {
        WindowSpec::empty().order_by_desc(col)
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
#[derive(Debug, Clone, PartialEq)]
pub struct WindowExpr<T> {
    kind: WindowExprKind,
    window: WindowSpec,
    _result: PhantomData<fn() -> T>,
}

impl<T> WindowExpr<T> {
    /// Creates a window expression wrapping `kind` over `window`.
    pub fn from_expr(kind: ExprKind, window: WindowSpec) -> Self {
        Self {
            kind: WindowExprKind::Expr(kind),
            window,
            _result: PhantomData,
        }
    }

    /// Renders this window function and its `OVER (...)` clause to `sql`.
    pub fn render<D: Dialect>(&self, sql: &mut String, params: &mut Vec<SqlVal>) {
        match &self.kind {
            WindowExprKind::RowNumber => sql.push_str("ROW_NUMBER()"),
            WindowExprKind::Rank => sql.push_str("RANK()"),
            WindowExprKind::DenseRank => sql.push_str("DENSE_RANK()"),
            WindowExprKind::Lag { column, offset } => {
                sql.push_str("LAG(");
                render_expr::<D>(sql, params, column);
                if let Some(off) = offset {
                    let _ = write!(sql, ", {off}");
                }
                sql.push(')');
            }
            WindowExprKind::Lead { column, offset } => {
                sql.push_str("LEAD(");
                render_expr::<D>(sql, params, column);
                if let Some(off) = offset {
                    let _ = write!(sql, ", {off}");
                }
                sql.push(')');
            }
            WindowExprKind::Expr(expr) => {
                render_expr::<D>(sql, params, expr);
            }
        }
        sql.push_str(" OVER (");
        self.window.render::<D>(sql, params);
        sql.push(')');
    }
}

impl<T: 'static + Send + Sync> SelectTarget for WindowExpr<T> {
    type RustType = T;

    fn render<D: Dialect>(&self, sql: &mut String, params: &mut Vec<SqlVal>) {
        WindowExpr::render::<D>(self, sql, params);
    }
}

impl<Tables, T> SelectTargetInScope<Tables, At<0>> for WindowExpr<T> {
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
    pub fn over(self, window: WindowSpec) -> WindowExpr<T> {
        WindowExpr {
            kind: self.kind,
            window,
            _result: PhantomData,
        }
    }
}

/// A `LAG` or `LEAD` window function builder with optional offset configuration.
#[derive(Debug, Clone, PartialEq)]
pub struct LagLeadFn<T> {
    is_lag: bool,
    column: ExprKind,
    offset: Option<u32>,
    _result: PhantomData<fn() -> T>,
}

impl<T> LagLeadFn<T> {
    /// Sets the row offset (defaults to 1 if omitted).
    pub fn offset(mut self, offset: u32) -> Self {
        self.offset = Some(offset);
        self
    }

    /// Evaluates this function over `window`.
    pub fn over(self, window: WindowSpec) -> WindowExpr<T> {
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
        WindowExpr {
            kind,
            window,
            _result: PhantomData,
        }
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
pub fn lag<C: Column>(_col: C) -> LagLeadFn<Option<C::RustType>> {
    LagLeadFn {
        is_lag: true,
        column: C::expr_kind(),
        offset: None,
        _result: PhantomData,
    }
}

/// Starts a `LEAD(C)` window function builder for column `C`.
pub fn lead<C: Column>(_col: C) -> LagLeadFn<Option<C::RustType>> {
    LagLeadFn {
        is_lag: false,
        column: C::expr_kind(),
        offset: None,
        _result: PhantomData,
    }
}
