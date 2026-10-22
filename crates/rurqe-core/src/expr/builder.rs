//! The generic [`Expr`] type and its fluent construction and comparison methods.

use core::marker::PhantomData;

use super::ast::{CaseBuilder, ExprKind, Subquery, SubqueryOp};
use super::ops::IntoExpr;
use super::render::{parse_raw_sql, RawSqlError};
use super::scope::{MergeScope, Merged, Merged3};
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
/// Phantom marker carrying the result type, referenced scope, and aggregation
/// marker of an [`Expr`].
///
/// The alias keeps the phantom field readable at its declaration site, mirroring
/// the query and statement markers of the surrounding modules.
type ExprMarker<T, Scope, Aggr> = PhantomData<fn() -> (T, Scope, Aggr)>;

#[derive(Debug, Clone, PartialEq)]
pub struct Expr<T, Scope = (), Aggr = NonAggregate> {
    pub(super) kind: ExprKind,
    _result: ExprMarker<T, Scope, Aggr>,
}

impl<T, Scope, Aggr> Expr<T, Scope, Aggr> {
    pub(super) fn new(kind: ExprKind) -> Self {
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
    pub fn is_null(&self) -> Expr<bool, Scope> {
        Expr::new(ExprKind::IsNull {
            operand: Box::new(self.kind.clone()),
            negated: false,
        })
    }

    /// Builds an `IS NOT NULL` predicate over this expression.
    pub fn is_not_null(&self) -> Expr<bool, Scope> {
        Expr::new(ExprKind::IsNull {
            operand: Box::new(self.kind.clone()),
            negated: true,
        })
    }

    /// Builds an `IN (subquery)` predicate matching this expression.
    ///
    /// The nested query renders inline with the enclosing statement, so its
    /// parameters continue the placeholder numbering of the outer query.
    pub fn in_subquery<S: Into<Subquery>>(self, subquery: S) -> Expr<bool, Scope> {
        Expr::new(ExprKind::Subquery {
            op: SubqueryOp::In,
            operand: Some(Box::new(self.kind)),
            query: subquery.into(),
            negated: false,
        })
    }

    /// Builds a `NOT IN (subquery)` predicate matching this expression.
    pub fn not_in_subquery<S: Into<Subquery>>(self, subquery: S) -> Expr<bool, Scope> {
        Expr::new(ExprKind::Subquery {
            op: SubqueryOp::In,
            operand: Some(Box::new(self.kind)),
            query: subquery.into(),
            negated: true,
        })
    }

    /// Compares the expression with `value` using the SQL `=` operator.
    pub fn eq<V>(self, value: V) -> Expr<bool, Merged<Scope, V::Scope>>
    where
        Scope: MergeScope<V::Scope>,
        V: IntoExpr<T>,
    {
        self.compare(BinaryOp::Eq, value.into_expr_kind())
    }

    /// Compares the expression with `value` using the SQL `<>` operator.
    pub fn ne<V>(self, value: V) -> Expr<bool, Merged<Scope, V::Scope>>
    where
        Scope: MergeScope<V::Scope>,
        V: IntoExpr<T>,
    {
        self.compare(BinaryOp::Ne, value.into_expr_kind())
    }

    /// Compares the expression with `value` using the SQL `>` operator.
    pub fn gt<V>(self, value: V) -> Expr<bool, Merged<Scope, V::Scope>>
    where
        Scope: MergeScope<V::Scope>,
        V: IntoExpr<T>,
    {
        self.compare(BinaryOp::Gt, value.into_expr_kind())
    }

    /// Compares the expression with `value` using the SQL `<` operator.
    pub fn lt<V>(self, value: V) -> Expr<bool, Merged<Scope, V::Scope>>
    where
        Scope: MergeScope<V::Scope>,
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
    ) -> Expr<bool, Merged3<Scope, L::Scope, H::Scope>>
    where
        Scope: MergeScope<L::Scope>,
        Merged<Scope, L::Scope>: MergeScope<H::Scope>,
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
    /// Every candidate value must be compatible with the expression type.
    pub fn in_list<I, V>(self, values: I) -> Expr<bool, Scope>
    where
        I: IntoIterator<Item = V>,
        V: IntoExpr<T>,
    {
        self.membership(values, false)
    }

    /// Builds a `NOT IN (value, ...)` negated membership predicate over this expression.
    pub fn not_in_list<I, V>(self, values: I) -> Expr<bool, Scope>
    where
        I: IntoIterator<Item = V>,
        V: IntoExpr<T>,
    {
        self.membership(values, true)
    }

    fn membership<I, V>(self, values: I, negated: bool) -> Expr<bool, Scope>
    where
        I: IntoIterator<Item = V>,
        V: IntoExpr<T>,
    {
        let values: Vec<ExprKind> = values
            .into_iter()
            .map(|value| value.into_expr_kind())
            .collect();

        if values.is_empty() {
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
            values,
            negated,
        })
    }

    fn compare<TargetScope>(self, op: BinaryOp, rhs: ExprKind) -> Expr<bool, TargetScope> {
        Expr::new(ExprKind::Binary {
            op,
            lhs: Box::new(self.kind),
            rhs: Box::new(rhs),
        })
    }

    /// Drops the scope tracking from this expression, enabling its use in correlated
    /// subqueries referencing outer query relations.
    pub fn without_scope(self) -> Expr<T, (), Aggr> {
        Expr {
            kind: self.kind,
            _result: PhantomData,
        }
    }

    /// Builds a window expression from this expression over `window`.
    ///
    /// The produced expression references the tables of both this expression and
    /// the window specification, so a window reading a table the query does not
    /// select from is rejected at the call site.
    pub fn over<WinScope>(
        self,
        window: crate::query::WindowSpec<WinScope>,
    ) -> crate::query::WindowExpr<T, Merged<Scope, WinScope>>
    where
        Scope: MergeScope<WinScope>,
    {
        crate::query::WindowExpr::of_window(
            crate::query::WindowExprKind::Expr(self.kind),
            window,
        )
    }

    /// Starts a `LAG(C)` window function builder for column `C`.
    ///
    /// The builder carries the table of `C`, which the resulting window
    /// expression merges with the scope of its `OVER` specification.
    pub fn lag<C: Column<RustType = T>>(col: C) -> crate::query::LagLeadFn<Option<T>, C::Table> {
        crate::query::lag(col)
    }

    /// Starts a `LEAD(C)` window function builder for column `C`.
    ///
    /// The builder carries the table of `C`, which the resulting window
    /// expression merges with the scope of its `OVER` specification.
    pub fn lead<C: Column<RustType = T>>(col: C) -> crate::query::LagLeadFn<Option<T>, C::Table> {
        crate::query::lead(col)
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
    /// its parameters continue the placeholder numbering of the outer query:
    ///
    /// ```
    /// use rurqe_core::dialect::Dialect;
    /// use rurqe_core::expr::Expr;
    /// use rurqe_core::query::SelectQuery;
    ///
    /// struct MockDialect;
    ///
    /// impl Dialect for MockDialect {
    ///     fn push_quoted_identifier(out: &mut String, ident: &str) {
    ///         out.push('"');
    ///         out.push_str(ident);
    ///         out.push('"');
    ///     }
    ///
    ///     fn push_placeholder(out: &mut String, index: usize) {
    ///         out.push('$');
    ///         out.push_str(&index.to_string());
    ///     }
    /// }
    ///
    /// rurqe_core::table! {
    ///     User => "users" {
    ///         UserId => "id": i64,
    ///         UserScore => "score": f64,
    ///     }
    /// }
    ///
    /// rurqe_core::table! {
    ///     Order => "orders" {
    ///         OrderUserId => "user_id": i64,
    ///         OrderTotal => "total": f64,
    ///     }
    /// }
    ///
    /// let biggest = SelectQuery::<MockDialect>::new()
    ///     .from::<Order>()
    ///     .select((OrderTotal,))
    ///     .where_(Expr::column::<OrderUserId>().eq(1i64));
    ///
    /// let query = SelectQuery::<MockDialect>::new()
    ///     .from::<User>()
    ///     .select((UserId,))
    ///     .where_(Expr::column::<UserScore>().eq(biggest));
    ///
    /// assert_eq!(
    ///     query.build().sql,
    ///     "SELECT \"id\" FROM \"users\" WHERE (\"score\" = (SELECT \"total\" FROM \"orders\" WHERE (\"user_id\" = $1)))"
    /// );
    /// ```
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
    pub fn subquery<D, Tables>(query: SelectQuery<D, Tables, (T,), HasProjection>) -> Self
    where
        D: Dialect,
        Tables: TableScope,
    {
        Self::new(ExprKind::Subquery {
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
    /// projectable from a grouped query even when `C` is not grouped.
    pub fn sum<C>() -> Expr<T, (), Aggregate>
    where
        C: Column<RustType = T>,
    {
        Expr::new(ExprKind::Aggregate {
            func: AggregateFunc::Sum,
            arg: Some(Box::new(C::expr_kind())),
        })
    }

    /// Builds a `MIN(C)` expression for column `C`.
    ///
    /// The produced expression carries the [`Aggregate`] marker, so it stays
    /// projectable from a grouped query even when `C` is not grouped.
    pub fn min<C>() -> Expr<T, (), Aggregate>
    where
        C: Column<RustType = T>,
    {
        Expr::new(ExprKind::Aggregate {
            func: AggregateFunc::Min,
            arg: Some(Box::new(C::expr_kind())),
        })
    }

    /// Builds a `MAX(C)` expression for column `C`.
    ///
    /// The produced expression carries the [`Aggregate`] marker, so it stays
    /// projectable from a grouped query even when `C` is not grouped.
    pub fn max<C>() -> Expr<T, (), Aggregate>
    where
        C: Column<RustType = T>,
    {
        Expr::new(ExprKind::Aggregate {
            func: AggregateFunc::Max,
            arg: Some(Box::new(C::expr_kind())),
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
