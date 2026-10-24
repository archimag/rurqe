//! The generic [`Expr`] type and its fluent construction and comparison methods.

use core::marker::PhantomData;
use core::ops::{Add, Div, Mul, Rem, Sub};

use super::ast::{CaseBuilder, ExprKind, MergeDialect, MergedDialect, SubqueryOp};
use super::functions::SqlCastTarget;
use super::ops::{
    CombineAggr, CombinedAggr, InSubquery, IntoExpr, IntoExprList, Numeric, NumericOperand,
};
use super::qualify::qualify_references;
use super::render::{parse_raw_sql, RawSqlError};
use super::scope::{MergeScope, Merged, Merged3, Unscoped};
use super::val::{AggregateFunc, BinaryOp, SqlVal};
use crate::dialect::Dialect;
use crate::query::{HasProjection, SelectQuery, TableScope};
use crate::schema::{Column, Table};

/// Marker of an expression that does not aggregate the rows of the query.
///
/// Column references, literals, raw SQL, and the composite expressions built
/// from them keep this marker. A grouped query therefore accepts them only when
/// they reference no table, where they stay constant within a group.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NonAggregate;

/// Marker of an expression produced by an aggregate function such as `SUM` or `COUNT`.
///
/// Aggregates collapse the rows of a group into a single value, so a grouped
/// query accepts them even when they reference a column the query does not group
/// by.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Aggregate;

/// A SQL expression whose result has the Rust type `T`, which references the
/// tables named by `Scope`, and which carries the aggregation marker `Aggr`.
///
/// Column references carry the table of their column, so a projection built from
/// a column of another table is rejected at the call site. Constructed values
/// (literals, raw SQL, `CASE` branches) reference no table and keep the default
/// scope `()`. Aggregate functions produce [`Aggregate`] expressions instead of
/// the default [`NonAggregate`] ones, which is what admits them into the
/// projection of a grouped query.
///
/// ```
/// use rurqe_core::expr::{BinaryOp, Expr, ExprKind, SqlVal};
///
/// rurqe_core::table! {
///     /// The `users` table.
///     User => "users" {
///         UserId => "id": i64,
///     }
/// }
///
/// let predicate = Expr::column::<UserId>().gt(10i64);
///
/// assert_eq!(
///     predicate.kind(),
///     &ExprKind::Binary {
///         op: BinaryOp::Gt,
///         lhs: Box::new(ExprKind::Column { table: "users", name: "id" }),
///         rhs: Box::new(ExprKind::Value(SqlVal::I64(10))),
///     }
/// );
/// ```
/// Phantom marker carrying the result type, referenced scope, aggregation marker,
/// and dialect marker of an [`Expr`].
///
/// The alias keeps the phantom field readable at its declaration site, mirroring
/// the query and statement markers of the surrounding modules.
type ExprMarker<T, Scope, Aggr, D> = PhantomData<fn() -> (T, Scope, Aggr, D)>;

#[derive(Debug, Clone, PartialEq)]
pub struct Expr<T, Scope = (), Aggr = NonAggregate, D = ()> {
    pub(super) kind: ExprKind,
    _result: ExprMarker<T, Scope, Aggr, D>,
}

impl<T, Scope, Aggr, D> Expr<T, Scope, Aggr, D> {
    pub(crate) fn new(kind: ExprKind) -> Self {
        Self {
            kind,
            _result: PhantomData,
        }
    }

    /// Builds an expression from an already assembled tree carrying the scope `Scope`.
    ///
    /// Dialect extensions describe their operands as AST nodes instead of
    /// pre-rendered text, so a column reference keeps following the statement it
    /// is rendered into: it is qualified in a multi-table query and keeps its
    /// alias prefix.
    #[doc(hidden)]
    pub fn from_kind(kind: ExprKind) -> Self {
        Self::new(kind)
    }

    /// The expression tree of this expression.
    pub fn kind(&self) -> &ExprKind {
        &self.kind
    }

    /// Consumes the expression and returns its tree.
    pub fn into_kind(self) -> ExprKind {
        self.kind
    }

    /// Builds an `IS NULL` predicate over this expression.
    ///
    /// The predicate compares against SQL `NULL` itself instead of a bound
    /// parameter, so it works for any expression type, optional or not.
    pub fn is_null(&self) -> Expr<bool, Scope, Aggr, D> {
        Expr::new(ExprKind::IsNull {
            operand: Box::new(self.kind.clone()),
            negated: false,
        })
    }

    /// Builds an `IS NOT NULL` predicate over this expression.
    pub fn is_not_null(&self) -> Expr<bool, Scope, Aggr, D> {
        Expr::new(ExprKind::IsNull {
            operand: Box::new(self.kind.clone()),
            negated: true,
        })
    }

    /// Builds an `IN (subquery)` predicate matching this expression.
    ///
    /// The nested query renders inline with the enclosing statement, so its
    /// parameters continue the placeholder numbering of the outer query.
    pub fn in_subquery<S: InSubquery>(
        self,
        subquery: S,
    ) -> Expr<bool, Scope, Aggr, MergedDialect<D, S::Dialect>>
    where
        D: MergeDialect<S::Dialect>,
    {
        Expr::new(ExprKind::Subquery {
            op: SubqueryOp::In,
            operand: Some(Box::new(self.kind)),
            query: subquery.into_subquery(),
            negated: false,
        })
    }

    /// Builds a `NOT IN (subquery)` predicate matching this expression.
    pub fn not_in_subquery<S: InSubquery>(
        self,
        subquery: S,
    ) -> Expr<bool, Scope, Aggr, MergedDialect<D, S::Dialect>>
    where
        D: MergeDialect<S::Dialect>,
    {
        Expr::new(ExprKind::Subquery {
            op: SubqueryOp::In,
            operand: Some(Box::new(self.kind)),
            query: subquery.into_subquery(),
            negated: true,
        })
    }

    /// Compares the expression with `value` using the SQL `=` operator.
    #[allow(clippy::type_complexity)]
    pub fn eq<V>(
        self,
        value: V,
    ) -> Expr<bool, Merged<Scope, V::Scope>, Aggr, MergedDialect<D, V::Dialect>>
    where
        Scope: MergeScope<V::Scope>,
        D: MergeDialect<V::Dialect>,
        V: IntoExpr<T>,
    {
        self.compare(BinaryOp::Eq, value.into_expr_kind())
    }

    /// Compares the expression with `value` using the SQL `<>` operator.
    #[allow(clippy::type_complexity)]
    pub fn ne<V>(
        self,
        value: V,
    ) -> Expr<bool, Merged<Scope, V::Scope>, Aggr, MergedDialect<D, V::Dialect>>
    where
        Scope: MergeScope<V::Scope>,
        D: MergeDialect<V::Dialect>,
        V: IntoExpr<T>,
    {
        self.compare(BinaryOp::Ne, value.into_expr_kind())
    }

    /// Compares the expression with `value` using the SQL `>` operator.
    #[allow(clippy::type_complexity)]
    pub fn gt<V>(
        self,
        value: V,
    ) -> Expr<bool, Merged<Scope, V::Scope>, Aggr, MergedDialect<D, V::Dialect>>
    where
        Scope: MergeScope<V::Scope>,
        D: MergeDialect<V::Dialect>,
        V: IntoExpr<T>,
    {
        self.compare(BinaryOp::Gt, value.into_expr_kind())
    }

    /// Compares the expression with `value` using the SQL `<` operator.
    #[allow(clippy::type_complexity)]
    pub fn lt<V>(
        self,
        value: V,
    ) -> Expr<bool, Merged<Scope, V::Scope>, Aggr, MergedDialect<D, V::Dialect>>
    where
        Scope: MergeScope<V::Scope>,
        D: MergeDialect<V::Dialect>,
        V: IntoExpr<T>,
    {
        self.compare(BinaryOp::Lt, value.into_expr_kind())
    }

    /// Builds a `BETWEEN low AND high` range predicate over this expression.
    ///
    /// Both bounds are inclusive and must be compatible with the expression type.
    #[allow(clippy::type_complexity)]
    pub fn between<L, H>(
        self,
        low: L,
        high: H,
    ) -> Expr<
        bool,
        Merged3<Scope, L::Scope, H::Scope>,
        Aggr,
        MergedDialect<MergedDialect<D, L::Dialect>, H::Dialect>,
    >
    where
        Scope: MergeScope<L::Scope>,
        Merged<Scope, L::Scope>: MergeScope<H::Scope>,
        D: MergeDialect<L::Dialect>,
        MergedDialect<D, L::Dialect>: MergeDialect<H::Dialect>,
        L: IntoExpr<T>,
        H: IntoExpr<T>,
    {
        Expr::new(ExprKind::Between {
            operand: Box::new(self.kind),
            low: Box::new(low.into_expr_kind()),
            high: Box::new(high.into_expr_kind()),
        })
    }

    /// Builds an `IN (value, ...)` membership predicate over this expression.
    ///
    /// Every candidate value must be compatible with the expression type, and the
    /// tables the candidates reference join the scope of the predicate: a list of
    /// bound values references no table and leaves the scope unchanged, while a
    /// candidate reading a column of another table extends it, so the enclosing
    /// query has to select from that table as well.
    #[allow(clippy::type_complexity)]
    pub fn in_list<I>(
        self,
        values: I,
    ) -> Expr<bool, Merged<Scope, I::Scope>, Aggr, MergedDialect<D, I::Dialect>>
    where
        Scope: MergeScope<I::Scope>,
        D: MergeDialect<I::Dialect>,
        I: IntoExprList<T>,
    {
        self.membership(values, false)
    }

    /// Builds a `NOT IN (value, ...)` negated membership predicate over this expression.
    ///
    /// The negation keeps the scope of the positive form: every candidate value
    /// still has to be comparable with the expression, and the tables it
    /// references still join the scope of the predicate.
    #[allow(clippy::type_complexity)]
    pub fn not_in_list<I>(
        self,
        values: I,
    ) -> Expr<bool, Merged<Scope, I::Scope>, Aggr, MergedDialect<D, I::Dialect>>
    where
        Scope: MergeScope<I::Scope>,
        D: MergeDialect<I::Dialect>,
        I: IntoExprList<T>,
    {
        self.membership(values, true)
    }

    #[allow(clippy::type_complexity)]
    fn membership<I>(
        self,
        values: I,
        negated: bool,
    ) -> Expr<bool, Merged<Scope, I::Scope>, Aggr, MergedDialect<D, I::Dialect>>
    where
        Scope: MergeScope<I::Scope>,
        D: MergeDialect<I::Dialect>,
        I: IntoExprList<T>,
    {
        let mut candidates = Vec::new();
        values.push_expr_kinds(&mut candidates);

        if candidates.is_empty() {
            // `x IN ()` is a syntax error in every dialect, so an empty
            // membership degrades into a constant predicate: nothing is a member
            // of an empty set, and everything is absent from it.
            return Expr::new(ExprKind::Raw {
                fragments: vec![String::from(if negated { "1 = 1" } else { "1 = 0" })],
                params: Vec::new(),
            });
        }

        Expr::new(ExprKind::InList {
            operand: Box::new(self.kind),
            values: candidates,
            negated,
        })
    }

    fn compare<TargetScope, TargetDialect>(
        self,
        op: BinaryOp,
        rhs: ExprKind,
    ) -> Expr<bool, TargetScope, Aggr, TargetDialect> {
        Expr::new(ExprKind::Binary {
            op,
            lhs: Box::new(self.kind),
            rhs: Box::new(rhs),
        })
    }

    /// Drops the relation scope from this expression, enabling its use in
    /// correlated subqueries referencing outer query relations.
    ///
    /// The dropped tables are replaced with the [`Unscoped`] marker, which is
    /// accepted in every query scope just like the empty scope `()` of a constant
    /// expression, yet stays distinguishable from it: a check relying on the
    /// difference — the projection rule of a grouped query, for instance — keeps
    /// treating the expression as one that reads a column.
    ///
    /// Every bare column reference of the tree becomes an explicit
    /// `"table"."column"` one while the scope is dropped. A correlated expression
    /// is rendered inside the statement of somebody else, which may address a
    /// single relation — the very table the reference reads, or that table under
    /// an alias the reference cannot name. The qualifier keeps the reference
    /// naming its own table instead of silently reading a column of the statement
    /// it is rendered into.
    pub fn without_scope(self) -> Expr<T, Unscoped, Aggr, D> {
        Expr {
            kind: qualify_references(self.kind),
            _result: PhantomData,
        }
    }

    /// Builds a window expression from this expression over `window`.
    ///
    /// The produced expression references the tables of both this expression and
    /// the window specification, so a window reading a table the query does not
    /// select from is rejected at the call site.
    pub fn over<WinScope, WinCols>(
        self,
        window: crate::query::WindowSpec<WinScope, WinCols>,
    ) -> crate::query::WindowExpr<T, Merged<Scope, WinScope>, WinCols>
    where
        Scope: MergeScope<WinScope>,
    {
        crate::query::WindowExpr::of_window(
            crate::query::WindowExprKind::Expr(self.kind),
            window,
        )
    }

    /// Converts the expression to the SQL type named by the cast target `U`.
    ///
    /// The conversion is rendered as `CAST(<expr> AS <type>)`, so the database
    /// performs it, and the expression keeps the tables it reads: a cast stays in
    /// the scope of its operand and remains projectable from a grouped query when
    /// that operand is an aggregate.
    ///
    /// ```
    /// use rurqe_core::expr::{Expr, ExprKind};
    ///
    /// rurqe_core::table! {
    ///     /// The `orders` table.
    ///     Order => "orders" {
    ///         OrderTotal => "total": f64,
    ///     }
    /// }
    ///
    /// let text = Expr::column::<OrderTotal>().cast::<String>();
    ///
    /// assert_eq!(
    ///     text.kind(),
    ///     &ExprKind::Cast {
    ///         operand: Box::new(ExprKind::Column {
    ///             table: "orders",
    ///             name: "total",
    ///         }),
    ///         target: "TEXT",
    ///     }
    /// );
    /// ```
    pub fn cast<U: SqlCastTarget>(self) -> Expr<U, Scope, Aggr, D> {
        Expr::new(ExprKind::Cast {
            operand: Box::new(self.kind),
            target: U::SQL_TYPE,
        })
    }

    /// Starts a `LAG(C)` window function builder for column `C`.
    ///
    /// The builder carries the table of `C`, which the resulting window
    /// expression merges with the scope of its `OVER` specification.
    pub fn lag<C: Column<RustType = T>>(
        col: C,
    ) -> crate::query::LagLeadFn<Option<T>, C::Table, crate::query::WindowColumnSet<C>> {
        crate::query::lag(col)
    }

    /// Starts a `LEAD(C)` window function builder for column `C`.
    ///
    /// The builder carries the table of `C`, which the resulting window
    /// expression merges with the scope of its `OVER` specification.
    pub fn lead<C: Column<RustType = T>>(
        col: C,
    ) -> crate::query::LagLeadFn<Option<T>, C::Table, crate::query::WindowColumnSet<C>> {
        crate::query::lead(col)
    }

    /// Builds an arithmetic expression applying `op` to this expression and `rhs`.
    ///
    /// The method backs the operator overloads of [`Expr`], so the operator is
    /// applied without inspecting the operands: the database evaluates the
    /// expression, which keeps an overflow or a division by zero the decision of
    /// the server instead of the client.
    fn arithmetic<TargetScope, TargetAggr, TargetDialect>(
        self,
        op: BinaryOp,
        rhs: ExprKind,
    ) -> Expr<T, TargetScope, TargetAggr, TargetDialect> {
        Expr::new(ExprKind::Binary {
            op,
            lhs: Box::new(self.kind),
            rhs: Box::new(rhs),
        })
    }
}

impl<T> Expr<T, ()> {
    /// Builds an expression holding the literal `value` typed as `T`.
    ///
    /// Literals are the constant operands of scalar function calls (see
    /// [`ExprKind::Function`]), where the value must carry the same type as the
    /// expression it accompanies.
    pub fn literal<V>(value: V) -> Self
    where
        V: IntoExpr<T>,
    {
        Self::new(value.into_expr_kind())
    }

    /// Starts a searched `CASE` expression whose branches produce values of type `T`.
    ///
    /// The builder starts in the [`NoBranches`](super::ast::NoBranches) state and
    /// only becomes finalisable once a [`CaseBuilder::when`] branch has been
    /// declared, so a `CASE` expression without branches does not compile.
    pub fn case() -> CaseBuilder<T> {
        CaseBuilder::new()
    }

    /// Builds a scalar subquery expression producing values of type `T`.
    ///
    /// The wrapped query must project exactly one column, so its output is the
    /// single-element tuple `(T,)`. A single-column `SelectQuery` converts into a
    /// scalar operand on its own, so most code passes the query directly to a
    /// comparison operator; this constructor spells the equivalent conversion out
    /// explicitly. The nested statement renders inline with the enclosing one, so
    /// its parameters continue the placeholder numbering of the outer query.
    ///
    /// A query projecting several columns is not a single value, so it does not
    /// convert into a scalar subquery:
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
    ///         UserName => "name": String,
    ///     }
    /// }
    ///
    /// // Fails to compile: the subquery projects two columns, not a single value.
    /// let _ = Expr::<i64>::subquery(
    ///     SelectQuery::<MockDialect>::new()
    ///         .from::<User>()
    ///         .select((UserId, UserName)),
    /// );
    /// ```
    pub fn subquery<D: Dialect, Tables: TableScope>(
        query: SelectQuery<D, Tables, (T,), HasProjection>,
    ) -> Expr<T, (), NonAggregate, D> {
        Expr::new(ExprKind::Subquery {
            op: SubqueryOp::Scalar,
            operand: None,
            query: query.into_subquery(),
            negated: false,
        })
    }

    /// Builds a raw SQL expression with bound parameters and dialect-aware placeholder rendering.
    ///
    /// The template uses `?` or `{}` placeholders — never both — and must declare
    /// exactly one placeholder per bound parameter; a literal question mark is
    /// escaped as `??` and counts as no placeholder. A template that does not
    /// match its parameters is reported as [`RawSqlError`] instead of being
    /// rendered into invalid SQL.
    pub fn raw(template: &str, params: Vec<SqlVal>) -> Result<Self, RawSqlError> {
        let fragments = parse_raw_sql(template, params.len())?;
        Ok(Self::new(ExprKind::Raw { fragments, params }))
    }

    /// Builds a raw SQL expression from fragments split by the `sql!` macro.
    ///
    /// The `sql!` and `raw_sql!` macros validate the placeholder census of their
    /// template at compile time and split it into literal text fragments, so the
    /// constructor only binds the parameters and the runtime never re-parses the
    /// template on each call.
    #[doc(hidden)]
    pub fn raw_fragments(fragments: Vec<String>, params: Vec<SqlVal>) -> Self {
        Self::new(ExprKind::Raw { fragments, params })
    }

    /// Builds a raw SQL expression carrying an explicit relation scope.
    #[doc(hidden)]
    pub fn raw_fragments_with_scope<Scope>(
        fragments: Vec<String>,
        params: Vec<SqlVal>,
    ) -> Expr<T, Scope> {
        Expr::new(ExprKind::Raw { fragments, params })
    }

    /// Builds a `SUM(C)` expression for column `C`.
    ///
    /// The produced expression carries the [`Aggregate`] marker, so it stays
    /// projectable from a grouped query even when `C` is not grouped. It also
    /// references the table of `C`, so a statement whose scope lacks that table
    /// rejects it like any other column reference.
    pub fn sum<C>() -> Expr<T, C::Table, Aggregate>
    where
        C: Column<RustType = T>,
    {
        Expr::new(ExprKind::Aggregate {
            func: AggregateFunc::Sum,
            distinct: false,
            arg: Some(Box::new(C::expr_kind())),
            filter: None,
        })
    }

    /// Builds a `SUM(DISTINCT C)` expression for column `C`.
    ///
    /// The scope is the table of `C` in both the distinct and the plain form, so
    /// the aggregate is checked against the statement it is projected into.
    pub fn sum_distinct<C>() -> Expr<T, C::Table, Aggregate>
    where
        C: Column<RustType = T>,
    {
        Expr::new(ExprKind::Aggregate {
            func: AggregateFunc::Sum,
            distinct: true,
            arg: Some(Box::new(C::expr_kind())),
            filter: None,
        })
    }

    /// Builds a `MIN(C)` expression for column `C`.
    ///
    /// The produced expression carries the [`Aggregate`] marker, so it stays
    /// projectable from a grouped query even when `C` is not grouped. It also
    /// references the table of `C`, so a statement whose scope lacks that table
    /// rejects it like any other column reference.
    pub fn min<C>() -> Expr<T, C::Table, Aggregate>
    where
        C: Column<RustType = T>,
    {
        Expr::new(ExprKind::Aggregate {
            func: AggregateFunc::Min,
            distinct: false,
            arg: Some(Box::new(C::expr_kind())),
            filter: None,
        })
    }

    /// Builds a `MAX(C)` expression for column `C`.
    ///
    /// The produced expression carries the [`Aggregate`] marker, so it stays
    /// projectable from a grouped query even when `C` is not grouped. It also
    /// references the table of `C`, so a statement whose scope lacks that table
    /// rejects it like any other column reference.
    pub fn max<C>() -> Expr<T, C::Table, Aggregate>
    where
        C: Column<RustType = T>,
    {
        Expr::new(ExprKind::Aggregate {
            func: AggregateFunc::Max,
            distinct: false,
            arg: Some(Box::new(C::expr_kind())),
            filter: None,
        })
    }
}

/// Marker of an aggregate expression restricted by a `FILTER (WHERE ...)` clause.
///
/// A filtered aggregate collapses the rows of its group exactly like a plain
/// aggregate; the marker only records that the aggregate no longer reads every
/// row of the group. Arithmetic combinations with constants preserve it, so a
/// filtered aggregate scaled by a value stays projectable from a grouped query.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FilteredAggregate;

/// A marker stating that an expression collapses the rows of its group.
///
/// The marker is implemented by [`Aggregate`] and [`FilteredAggregate`], so the
/// projection and filtering rules of a grouped query accept both kinds of
/// aggregate.
#[doc(hidden)]
pub trait AggregateMarker {}

impl AggregateMarker for Aggregate {}

impl AggregateMarker for FilteredAggregate {}

impl<T, Scope, Marker, D> Expr<T, Scope, Marker, D>
where
    Marker: AggregateMarker,
{
    /// Marks the aggregate expression to evaluate only over `DISTINCT` values.
    pub fn distinct(mut self) -> Self {
        if let ExprKind::Aggregate {
            ref mut distinct, ..
        } = self.kind
        {
            *distinct = true;
        }
        self
    }

    /// Adds a `FILTER (WHERE ...)` clause restricting the rows this aggregate evaluates.
    ///
    /// Multiple calls combine their conditions with the SQL `AND` operator, and
    /// the expression records the filter in its tree instead of in the statement
    /// that renders it. The aggregate therefore stays a dialect-neutral value and
    /// the caller chooses the dialect when rendering the statement it is
    /// projected into.
    ///
    /// The produced expression carries [`FilteredAggregate`], so the projection
    /// and filtering rules of a grouped query keep accepting it while arithmetic
    /// combinations with constants preserve the marker.
    ///
    /// ```
    /// use rurqe_core::expr::{sum, Expr, ExprKind};
    ///
    /// rurqe_core::table! {
    ///     Order => "orders" {
    ///         OrderTotal => "total": f64,
    ///     }
    /// }
    ///
    /// let filtered = sum(OrderTotal)
    ///     .filter_where(Expr::column::<OrderTotal>().gt(10.0))
    ///     .filter_where(Expr::column::<OrderTotal>().lt(100.0));
    ///
    /// assert!(matches!(
    ///     filtered.kind(),
    ///     ExprKind::Aggregate { filter: Some(_), .. }
    /// ));
    /// ```
    pub fn filter_where<P, PredAggr, PredD>(
        self,
        predicate: Expr<bool, P, PredAggr, PredD>,
    ) -> Expr<T, Merged<Scope, P>, FilteredAggregate, MergedDialect<D, PredD>>
    where
        Scope: MergeScope<P>,
        D: MergeDialect<PredD>,
    {
        let ExprKind::Aggregate {
            func,
            distinct,
            arg,
            filter,
        } = self.kind
        else {
            return Expr::new(self.kind);
        };

        let new_filter = match filter {
            Some(existing) => Box::new(ExprKind::Binary {
                op: BinaryOp::And,
                lhs: existing,
                rhs: Box::new(predicate.into_kind()),
            }),
            None => Box::new(predicate.into_kind()),
        };

        Expr::new(ExprKind::Aggregate {
            func,
            distinct,
            arg,
            filter: Some(new_filter),
        })
    }
}

impl Expr<(), ()> {
    /// Builds an expression referencing the column `C`.
    ///
    /// The column fixes both the result type and the expression scope, so the
    /// returned expression is checked against the query scope at the call site.
    /// Columns of an aliased table build an alias-qualified reference, so
    /// `Expr::column::<AliasedCol<A, C>>()` renders as `"alias"."column"`.
    pub fn column<C>() -> Expr<C::RustType, C::Table>
    where
        C: Column,
    {
        Expr::new(C::expr_kind())
    }

    /// Builds an expression referencing the column `C` qualified by its table `Q`.
    ///
    /// Qualified references render as `"table"."column"`, which keeps joins over
    /// tables that share a column name unambiguous.
    pub fn qualified_column<Q, C>() -> Expr<C::RustType, Q>
    where
        Q: Table,
        C: Column<Table = Q>,
    {
        Expr::new(ExprKind::QualifiedColumn {
            table: Q::NAME,
            column: C::NAME,
        })
    }
}

// Arithmetic follows the standard operator traits, so a numeric expression
// composes the way the SQL it renders reads: `Expr::column::<OrderTotal>() + 1.5`
// renders `("total" + $1)`. The database evaluates the operator, which is why the
// operands only have to carry the same numeric type and no value is folded on the
// client. The aggregation marker of the produced expression is derived by
// `CombineAggr` from the marker of the left operand and the operand on the right,
// so an aggregate scaled by a constant stays projectable from a grouped query,
// while an aggregate combined with a column read per row stops being one:
//
// ```text
// SUM("total") + $1     -> an aggregate
// SUM("total") + "tax"  -> one value per row
// ```

impl<T, Scope, Aggr, D, Rhs> Add<Rhs> for Expr<T, Scope, Aggr, D>
where
    T: Numeric,
    Scope: MergeScope<Rhs::Scope>,
    D: MergeDialect<Rhs::Dialect>,
    Rhs: NumericOperand<T>,
    Aggr: CombineAggr<Rhs::Aggr, Rhs::Scope>,
{
    type Output = Expr<
        T,
        Merged<Scope, Rhs::Scope>,
        CombinedAggr<Aggr, Rhs::Aggr, Rhs::Scope>,
        MergedDialect<D, Rhs::Dialect>,
    >;

    fn add(self, rhs: Rhs) -> Self::Output {
        self.arithmetic(BinaryOp::Add, rhs.into_operand_kind())
    }
}

impl<T, Scope, Aggr, D, Rhs> Sub<Rhs> for Expr<T, Scope, Aggr, D>
where
    T: Numeric,
    Scope: MergeScope<Rhs::Scope>,
    D: MergeDialect<Rhs::Dialect>,
    Rhs: NumericOperand<T>,
    Aggr: CombineAggr<Rhs::Aggr, Rhs::Scope>,
{
    type Output = Expr<
        T,
        Merged<Scope, Rhs::Scope>,
        CombinedAggr<Aggr, Rhs::Aggr, Rhs::Scope>,
        MergedDialect<D, Rhs::Dialect>,
    >;

    fn sub(self, rhs: Rhs) -> Self::Output {
        self.arithmetic(BinaryOp::Sub, rhs.into_operand_kind())
    }
}

impl<T, Scope, Aggr, D, Rhs> Mul<Rhs> for Expr<T, Scope, Aggr, D>
where
    T: Numeric,
    Scope: MergeScope<Rhs::Scope>,
    D: MergeDialect<Rhs::Dialect>,
    Rhs: NumericOperand<T>,
    Aggr: CombineAggr<Rhs::Aggr, Rhs::Scope>,
{
    type Output = Expr<
        T,
        Merged<Scope, Rhs::Scope>,
        CombinedAggr<Aggr, Rhs::Aggr, Rhs::Scope>,
        MergedDialect<D, Rhs::Dialect>,
    >;

    fn mul(self, rhs: Rhs) -> Self::Output {
        self.arithmetic(BinaryOp::Mul, rhs.into_operand_kind())
    }
}

impl<T, Scope, Aggr, D, Rhs> Div<Rhs> for Expr<T, Scope, Aggr, D>
where
    T: Numeric,
    Scope: MergeScope<Rhs::Scope>,
    D: MergeDialect<Rhs::Dialect>,
    Rhs: NumericOperand<T>,
    Aggr: CombineAggr<Rhs::Aggr, Rhs::Scope>,
{
    type Output = Expr<
        T,
        Merged<Scope, Rhs::Scope>,
        CombinedAggr<Aggr, Rhs::Aggr, Rhs::Scope>,
        MergedDialect<D, Rhs::Dialect>,
    >;

    fn div(self, rhs: Rhs) -> Self::Output {
        self.arithmetic(BinaryOp::Div, rhs.into_operand_kind())
    }
}

impl<T, Scope, Aggr, D, Rhs> Rem<Rhs> for Expr<T, Scope, Aggr, D>
where
    T: Numeric,
    Scope: MergeScope<Rhs::Scope>,
    D: MergeDialect<Rhs::Dialect>,
    Rhs: NumericOperand<T>,
    Aggr: CombineAggr<Rhs::Aggr, Rhs::Scope>,
{
    type Output = Expr<
        T,
        Merged<Scope, Rhs::Scope>,
        CombinedAggr<Aggr, Rhs::Aggr, Rhs::Scope>,
        MergedDialect<D, Rhs::Dialect>,
    >;

    fn rem(self, rhs: Rhs) -> Self::Output {
        self.arithmetic(BinaryOp::Rem, rhs.into_operand_kind())
    }
}
