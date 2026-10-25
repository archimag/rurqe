//! The generic [`Expr`] type together with its markers and its plain constructors.
//!
//! Expressions are typed by the Rust type of their result: referencing a column
//! yields an expression carrying the column's
//! [`RustType`](crate::schema::Column::RustType), while comparing it with a value
//! yields a boolean predicate. Operand compatibility is checked at the call site
//! through [`SqlCompatible`](crate::expr::SqlCompatible), so a comparison between
//! unrelated types simply does not compile.
//!
//! The fluent surface is split along the concerns it serves: [`predicates`] turns
//! an expression into a comparison, a range, a membership test, or a null test,
//! [`aggregates`] builds the aggregates and carries the markers the grouped-query
//! rules rely on, and [`arithmetic`] overloads the arithmetic operators. This
//! module keeps the [`Expr`] type itself, its aggregation markers, and the
//! constructors that only assemble a node.

mod aggregates;
mod arithmetic;
mod predicates;

pub use aggregates::{AcceptsAggregateClause, AggregateMarker, DirectAggregate, FilteredAggregate};

use core::marker::PhantomData;

use super::ast::{CaseBuilder, ExprKind, MergeDialect, MergedDialect};
use super::functions::SqlCastTarget;
use super::ops::IntoExpr;
use super::qualify::qualify_references;
use super::render::{parse_raw_sql, RawSqlError};
use super::scope::{MergeScope, Merged, Unscoped};
use super::val::SqlVal;
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

/// Marker of an expression that collapses the rows of its group without keeping
/// the aggregate call at its root.
///
/// A direct aggregate call carries [`DirectAggregate`] and switches to
/// [`FilteredAggregate`] once a `FILTER (WHERE ...)` clause is attached; an
/// expression built from an aggregate — `SUM("total") + $1`,
/// `COALESCE(SUM("total"), $1)`, a `CASE` over aggregates — degrades to this
/// marker. All three collapse the rows of a group into a single value, so a
/// grouped query accepts them even when they reference a column the query does not
/// group by, while `DISTINCT` and `FILTER (WHERE ...)` stay reachable on the
/// aggregate call alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Aggregate;

/// Phantom marker carrying the result type, referenced scope, aggregation marker,
/// and dialect marker of an [`Expr`].
///
/// The alias keeps the phantom field readable at its declaration site, mirroring
/// the query and statement markers of the surrounding modules.
type ExprMarker<T, Scope, Aggr, D> = PhantomData<fn() -> (T, Scope, Aggr, D)>;

/// A SQL expression whose result has the Rust type `T`, which references the
/// tables named by `Scope`, and which carries the aggregation marker `Aggr`.
///
/// Column references carry the table of their column, so a projection built from
/// a column of another table is rejected at the call site. Constructed values
/// (literals, raw SQL, `CASE` branches) reference no table and keep the default
/// scope `()`. Aggregate functions produce [`DirectAggregate`] expressions
/// instead of the default [`NonAggregate`] ones, and an expression combining them
/// carries [`Aggregate`]; either marker is what admits the projection into a
/// grouped query, while the `DISTINCT` and `FILTER (WHERE ...)` clauses of the
/// call stay reachable on the direct call alone.
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

    /// The expression tree of this expression.
    pub fn kind(&self) -> &ExprKind {
        &self.kind
    }

    /// Consumes the expression and returns its tree.
    pub fn into_kind(self) -> ExprKind {
        self.kind
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
        Expr::new(qualify_references(self.kind))
    }

    /// Builds a window expression from this expression over `window`.
    ///
    /// The produced expression references the tables of both this expression and
    /// the window specification, so a window reading a table the query does not
    /// select from is rejected at the call site. The dialect requirements of the
    /// two sides merge as well, so an ordering fixing the placement of NULL values
    /// keeps requiring a dialect that renders the modifier even when the
    /// expression it is evaluated over is dialect-neutral.
    pub fn over<WinScope, WinCols, WinD>(
        self,
        window: crate::query::WindowSpec<WinScope, WinCols, WinD>,
    ) -> crate::query::WindowExpr<T, Merged<Scope, WinScope>, WinCols, MergedDialect<WinD, D>>
    where
        Scope: MergeScope<WinScope>,
        WinD: MergeDialect<D>,
    {
        let window = window.retype_dialect::<D>();
        crate::query::WindowExpr::of_window(crate::query::WindowExprKind::Expr(self.kind), window)
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
}

impl<T, Scope, D> Expr<T, Scope, NonAggregate, D> {
    /// Builds an expression from an already assembled tree carrying the scope `Scope`.
    ///
    /// Dialect extensions describe their operands as AST nodes instead of
    /// pre-rendered text, so a column reference keeps following the statement it
    /// is rendered into: it is qualified in a multi-table query and keeps its
    /// alias prefix.
    ///
    /// The constructor builds per-row expressions only. The aggregate markers
    /// [`DirectAggregate`] and [`FilteredAggregate`] state that the root of the
    /// tree is the aggregate call itself — the property `distinct` and
    /// `filter_where` rely on to attach their clause to the call they modify — so
    /// an expression carrying one of them is assembled by the aggregate
    /// constructors, which build the call, instead of from an arbitrary tree.
    /// Forging the marker onto a non-aggregate tree does not compile:
    ///
    /// ```compile_fail
    /// use rurqe_core::expr::{DirectAggregate, Expr, ExprKind};
    ///
    /// rurqe_core::table! {
    ///     /// The `orders` table.
    ///     Order => "orders" {
    ///         OrderTotal => "total": f64,
    ///     }
    /// }
    ///
    /// // Fails to compile: the tree is a column reference, not an aggregate
    /// // call, so the `DirectAggregate` marker cannot be named on the
    /// // expression `from_kind` builds.
    /// let _: Expr<f64, Order, DirectAggregate> = Expr::from_kind(ExprKind::Column {
    ///     table: "orders",
    ///     name: "total",
    /// });
    /// ```
    ///
    /// A tree that computes one value per row keeps the constructor reachable:
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
    /// let expr: Expr<f64, Order> = Expr::from_kind(ExprKind::Column {
    ///     table: "orders",
    ///     name: "total",
    /// });
    ///
    /// assert_eq!(
    ///     expr.kind(),
    ///     &ExprKind::Column {
    ///         table: "orders",
    ///         name: "total",
    ///     }
    /// );
    /// ```
    #[doc(hidden)]
    pub fn from_kind(kind: ExprKind) -> Self {
        Self::new(kind)
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
            op: super::ast::SubqueryOp::Scalar,
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
    ///
    /// The fragments interleave with the parameters: the renderer emits the
    /// fragments in order and binds one parameter after every fragment except
    /// the last, so a caller has to supply exactly one more fragment than
    /// parameters — the shape [`parse_raw_sql`] splits a template into. The
    /// constructor binds the values without re-parsing them, so an unbalanced
    /// pair is not reported here; it is reported where it would dispatch SQL
    /// with a missing or a misplaced placeholder instead: the renderer checks
    /// the census while it emits the expression and returns
    /// [`RawSqlError::PlaceholderCount`], while the pre-flight validation of an
    /// enclosing statement rejects the expression before a single character
    /// reaches its buffers. A template that still needs splitting goes through
    /// [`raw`](Self::raw), which validates the census and returns a typed error.
    #[doc(hidden)]
    pub fn raw_fragments(fragments: Vec<String>, params: Vec<SqlVal>) -> Self {
        Self::new(ExprKind::Raw { fragments, params })
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
