//! Type-safe `SELECT` builder and its single-pass SQL renderer.
//!
//! A [`SelectQuery`] is parameterised by the target [`Dialect`], the table in
//! scope, and the Rust type of the produced row. Builder methods only change the
//! phantom parameters, so no SQL exists until [`SelectQuery::build`] performs a
//! deterministic rendering pass that returns the SQL text together with the
//! ordered list of bound parameters.
//!
//! ```text
//! SELECT "id", "name" FROM "users" WHERE ("id" > $1) LIMIT 10
//! ```
//!
//! Joining another table keeps the scope at the type level, so the rendered
//! `FROM` clause always matches the tables the query was built with:
//!
//! ```text
//! SELECT "id", "name" FROM "users" INNER JOIN "orders" ON ("user_id" = $1)
//! ```
//!
//! Rendering and execution are decoupled: [`ExecutableQuery`] exposes both the
//! rendered [`BuiltQuery`] and the row type of every query builder, while
//! [`QueryBackend`] translates the typed [`SqlVal`] parameters into native
//! driver values. [`QueryBackendExt`] adds `fetch_all`, `fetch_one`,
//! `fetch_optional`, and `execute`, decoding every returned row through
//! [`FromRow`].

use core::fmt;
use core::fmt::Write;
use core::marker::PhantomData;
use std::sync::Arc;

use crate::dialect::{Dialect, SupportsLateralJoin};
use crate::dml::{DeleteQuery, Filtered, InsertQuery, UpdateQuery};
use crate::expr::{BinaryOp, Expr, ExprKind, SqlVal, Subquery};
use crate::row::FromRow;
use crate::schema::{Column, ExprScope, Table, TableRef};

/// A single item projected by a `SELECT` query, such as a column or a window expression.
#[diagnostic::on_unimplemented(
    message = "`{Self}` is not a valid projection item",
    label = "must be a column, nullable column, or window expression",
    note = "columns, nullable columns, and window expressions (`.over(...)`) can be projected in a selection"
)]
pub trait SelectTarget {
    /// The Rust type produced for this projection.
    type RustType;

    /// Pushes the identifier name of this target if it is a simple column reference.
    fn push_column_name(&self, out: &mut Vec<&'static str>) {
        let _ = out;
    }

    /// Renders this projection item to the SQL buffer and binds any parameters.
    fn render<D: Dialect>(&self, sql: &mut String, params: &mut Vec<SqlVal>);
}

impl<C: Column> SelectTarget for C {
    type RustType = C::RustType;

    fn push_column_name(&self, out: &mut Vec<&'static str>) {
        out.push(C::NAME);
    }

    fn render<D: Dialect>(&self, sql: &mut String, params: &mut Vec<SqlVal>) {
        crate::expr::render_expr::<D>(sql, params, &C::expr_kind());
    }
}

impl<T: 'static + Send + Sync> SelectTarget for WindowExpr<T> {
    type RustType = T;

    fn render<D: Dialect>(&self, sql: &mut String, params: &mut Vec<SqlVal>) {
        WindowExpr::render::<D>(self, sql, params);
    }
}

impl<T: 'static + Send + Sync> SelectTarget for Expr<T> {
    type RustType = T;

    fn render<D: Dialect>(&self, sql: &mut String, params: &mut Vec<SqlVal>) {
        crate::expr::render_expr::<D>(sql, params, self.kind());
    }
}

/// A list of selected columns whose Rust value types form the query row type.
///
/// The trait is implemented for tuples of projection targets, so `(UserId,)`
/// yields a `(i64,)` row and `(UserId, row_number().over(w))` yields `(i64, i64)`.
#[diagnostic::on_unimplemented(
    message = "`{Self}` is not a supported selection",
    label = "select a column or a tuple of columns",
    note = "selections are column markers written as tuples, such as `(UserId,)` or `(UserId, UserName)`"
)]
pub trait Selection {
    /// The tuple of Rust values produced for a single row.
    type Row;

    /// Appends the SQL names of simple selected columns to `out`.
    fn push_column_names(&self, out: &mut Vec<&'static str>) {
        let _ = out;
    }

    /// Renders all projection items into `sql`, separating them by commas.
    fn render_projections<D: Dialect>(&self, sql: &mut String, _params: &mut Vec<SqlVal>) {
        let mut names = Vec::new();
        self.push_column_names(&mut names);
        for (index, name) in names.into_iter().enumerate() {
            if index > 0 {
                sql.push_str(", ");
            }
            D::push_quoted_identifier(sql, name);
        }
    }
}

macro_rules! impl_selection {
    ($($target:ident),+) => {
        #[allow(non_snake_case, unused_assignments)]
        impl<$($target: SelectTarget),+> Selection for ($($target,)+) {
            type Row = ($($target::RustType,)+);

            fn push_column_names(&self, out: &mut Vec<&'static str>) {
                let ($($target,)+) = self;
                $($target.push_column_name(out);)+
            }

            fn render_projections<D: Dialect>(&self, sql: &mut String, params: &mut Vec<SqlVal>) {
                let ($($target,)+) = self;
                let mut first = true;
                $(
                    if !first {
                        sql.push_str(", ");
                    }
                    first = false;
                    $target.render::<D>(sql, params);
                )+
            }
        }
    };
}

impl_selection!(T1);
impl_selection!(T1, T2);
impl_selection!(T1, T2, T3);
impl_selection!(T1, T2, T3, T4);
impl_selection!(T1, T2, T3, T4, T5);
impl_selection!(T1, T2, T3, T4, T5, T6);
impl_selection!(T1, T2, T3, T4, T5, T6, T7);
impl_selection!(T1, T2, T3, T4, T5, T6, T7, T8);

/// A column or tuple of columns for the `GROUP BY` clause.
#[diagnostic::on_unimplemented(
    message = "`{Self}` is not a valid GROUP BY clause",
    label = "group by a column or a tuple of columns",
    note = "group by accepts column markers or tuples of column markers, such as `UserId` or `(UserId, UserName)`"
)]
pub trait GroupBy {
    /// Appends the column names to `out` in grouping order.
    fn push_column_names(&self, out: &mut Vec<&'static str>);
}

impl<C: Column> GroupBy for C {
    fn push_column_names(&self, out: &mut Vec<&'static str>) {
        out.push(C::NAME);
    }
}

macro_rules! impl_group_by {
    ($($column:ident),+) => {
        impl<$($column: Column),+> GroupBy for ($($column,)+) {
            fn push_column_names(&self, out: &mut Vec<&'static str>) {
                $(out.push($column::NAME);)+
            }
        }
    };
}

impl_group_by!(C1);
impl_group_by!(C1, C2);
impl_group_by!(C1, C2, C3);
impl_group_by!(C1, C2, C3, C4);
impl_group_by!(C1, C2, C3, C4, C5);
impl_group_by!(C1, C2, C3, C4, C5, C6);
impl_group_by!(C1, C2, C3, C4, C5, C6, C7);
impl_group_by!(C1, C2, C3, C4, C5, C6, C7, C8);

/// The ordered list of tables a query selects from, joined tables included.
///
/// The trait is implemented for every [`Table`] and for each [`Joined`] scope
/// produced by [`SelectQuery::inner_join`], so the rendered `FROM` clause stays
/// in sync with the tables tracked at the type level.
#[diagnostic::on_unimplemented(
    message = "`{Self}` is not a valid query scope",
    label = "a query must select from a table",
    note = "introduce the first table with `SelectQuery::from` and additional tables with `SelectQuery::inner_join`"
)]
pub trait TableScope {
    /// Appends the table names of the scope in `FROM` order.
    fn push_table_names(out: &mut Vec<&'static str>);

    /// Appends the table references of the scope, aliases included, in `FROM` order.
    ///
    /// The default derives plain references from
    /// [`push_table_names`](TableScope::push_table_names), so a scope only
    /// overrides it when its tables are referenced under an alias.
    fn push_table_refs(out: &mut Vec<TableRef>) {
        let mut names = Vec::new();
        Self::push_table_names(&mut names);
        out.extend(names.into_iter().map(TableRef::plain));
    }
}

impl<T: Table> TableScope for T {
    fn push_table_names(out: &mut Vec<&'static str>) {
        out.push(T::NAME);
    }

    fn push_table_refs(out: &mut Vec<TableRef>) {
        out.push(T::table_ref());
    }
}

/// The flavour of join used to bring a table into the query scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum JoinKind {
    /// `INNER JOIN`, which only keeps rows matched in the joined table.
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

/// A single join clause recorded on a [`SelectQuery`].
///
/// Regular joins reference the table next in scope and carry their `ON`
/// condition, while lateral joins carry a renderer for the inline subquery
/// rendered after the `LATERAL` keyword.
#[derive(Clone)]
struct JoinClause {
    kind: JoinKind,
    on: Option<ExprKind>,
    lateral: Option<CteRenderer>,
}

/// A query scope extending `Tables` with the table `T` joined to it.
///
/// The marker is produced by [`SelectQuery::inner_join`], carries no values, and
/// only records the `FROM` order of the participating tables.
pub struct Joined<Tables, T>(PhantomData<fn() -> (Tables, T)>);

impl<Tables: TableScope, T: Table> TableScope for Joined<Tables, T> {
    fn push_table_names(out: &mut Vec<&'static str>) {
        Tables::push_table_names(out);
        out.push(T::NAME);
    }

    fn push_table_refs(out: &mut Vec<TableRef>) {
        Tables::push_table_refs(out);
        out.push(T::table_ref());
    }
}

/// A query scope extending `Tables` with the table `T` joined by a `LEFT JOIN`.
///
/// The marker is produced by [`SelectQuery::left_join`] and records the same
/// `FROM` order as [`Joined`], while signalling that the columns of `T` may be
/// absent from a result row.
pub struct LeftJoined<Tables, T>(PhantomData<fn() -> (Tables, T)>);

impl<Tables: TableScope, T: Table> TableScope for LeftJoined<Tables, T> {
    fn push_table_names(out: &mut Vec<&'static str>) {
        Tables::push_table_names(out);
        out.push(T::NAME);
    }

    fn push_table_refs(out: &mut Vec<TableRef>) {
        Tables::push_table_refs(out);
        out.push(T::table_ref());
    }
}

impl<Tables: ExprScope, T: Table> ExprScope for Joined<Tables, T> {}

impl<Tables: ExprScope, T: Table> ExprScope for LeftJoined<Tables, T> {}

/// A query scope extending `Tables` with the `LATERAL` subquery exposed as `T`.
///
/// The marker is produced by [`SelectQuery::cross_join_lateral`] and records the
/// `FROM` order of the participating relations, the inline subquery included.
pub struct CrossLateralJoined<Tables, T>(PhantomData<fn() -> (Tables, T)>);

impl<Tables: TableScope, T: Table> TableScope for CrossLateralJoined<Tables, T> {
    fn push_table_names(out: &mut Vec<&'static str>) {
        Tables::push_table_names(out);
        out.push(T::NAME);
    }

    fn push_table_refs(out: &mut Vec<TableRef>) {
        Tables::push_table_refs(out);
        out.push(T::table_ref());
    }
}

impl<Tables: ExprScope, T: Table> ExprScope for CrossLateralJoined<Tables, T> {}

/// A query scope extending `Tables` with a `LEFT JOIN LATERAL` subquery exposed as `T`.
///
/// The marker is produced by [`SelectQuery::left_join_lateral`] and signals that
/// the subquery may leave outer rows unmatched, so its columns are projected
/// through [`SelectQuery::nullable`] to keep their absence visible in the row
/// type.
pub struct LeftLateralJoined<Tables, T>(PhantomData<fn() -> (Tables, T)>);

impl<Tables: TableScope, T: Table> TableScope for LeftLateralJoined<Tables, T> {
    fn push_table_names(out: &mut Vec<&'static str>) {
        Tables::push_table_names(out);
        out.push(T::NAME);
    }

    fn push_table_refs(out: &mut Vec<TableRef>) {
        Tables::push_table_refs(out);
        out.push(T::table_ref());
    }
}

impl<Tables: ExprScope, T: Table> ExprScope for LeftLateralJoined<Tables, T> {}

/// A column lifted to its nullable form for outer joins.
///
/// The marker keeps the name and owning table of the column `C`, but replaces
/// its Rust type with `Option<C::RustType>`. Projecting a column of a
/// [`left_join`](SelectQuery::left_join)ed table through
/// [`SelectQuery::nullable`] therefore yields an optional value at compile time,
/// matching the `NULL` values produced by the database.
pub struct Nullable<C>(PhantomData<fn() -> C>);

impl<C: Column> Nullable<C> {
    /// Creates the nullable marker for the column `C`.
    pub fn new() -> Self {
        Nullable(PhantomData)
    }
}

impl<C: Column> Default for Nullable<C> {
    fn default() -> Self {
        Self::new()
    }
}

impl<C: Column> Column for Nullable<C> {
    type Table = C::Table;
    type RustType = Option<C::RustType>;

    const NAME: &'static str = C::NAME;

    fn expr_kind() -> ExprKind {
        C::expr_kind()
    }
}

/// The rendered SQL text of a query together with its bound parameters.
#[derive(Debug, Clone, PartialEq)]
pub struct BuiltQuery {
    /// The SQL text using the dialect's positional placeholders.
    pub sql: String,

    /// The parameter values referenced by the placeholders, in placeholder order.
    pub params: Vec<SqlVal>,
}

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

type CteRenderer = Arc<dyn Fn(&mut String, &mut Vec<SqlVal>) + Send + Sync>;

/// Internal definition of a single Common Table Expression (`WITH cte AS (...)`).
#[derive(Clone)]
struct CteDefinition {
    name: &'static str,
    renderer: CteRenderer,
}

/// A Common Table Expression (`WITH cte AS (...)`) encapsulating a subquery and its output shape.
#[derive(Clone)]
pub struct Cte<T = (), Row = ()> {
    name: &'static str,
    renderer: CteRenderer,
    _marker: PhantomData<fn() -> (T, Row)>,
}

impl<T, Row> fmt::Debug for Cte<T, Row> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Cte")
            .field("name", &self.name)
            .finish()
    }
}

impl<T: Table, Row> Cte<T, Row> {
    /// Creates a CTE for the table `T` from a `SelectQuery`.
    pub fn new<D: Dialect, S: TableScope + 'static>(
        query: SelectQuery<D, S, Row>,
    ) -> Self
    where
        Row: 'static,
    {
        Self {
            name: T::NAME,
            renderer: Arc::new(move |sql, params| {
                query.render_into(sql, params);
            }),
            _marker: PhantomData,
        }
    }
}

impl<Row> Cte<(), Row> {
    /// Creates a CTE with an explicit alias `name` from a `SelectQuery`.
    pub fn named<D: Dialect, S: TableScope + 'static>(
        name: &'static str,
        query: SelectQuery<D, S, Row>,
    ) -> Self
    where
        Row: 'static,
    {
        Self {
            name,
            renderer: Arc::new(move |sql, params| {
                query.render_into(sql, params);
            }),
            _marker: PhantomData,
        }
    }
}

impl<T, Row> Cte<T, Row> {
    /// Returns the SQL identifier / alias of this CTE.
    pub const fn name(&self) -> &'static str {
        self.name
    }
}

impl<T: Table, Row: 'static> Table for Cte<T, Row> {
    const NAME: &'static str = T::NAME;
}

/// Starts a CTE definition for the table marker `T` from `query`.
pub fn cte<T: Table, D: Dialect, S: TableScope + 'static, Row: 'static>(
    query: SelectQuery<D, S, Row>,
) -> Cte<T, Row> {
    Cte::new(query)
}

/// Starts a CTE definition with an explicit alias `name` from `query`.
pub fn cte_named<D: Dialect, S: TableScope + 'static, Row: 'static>(
    name: &'static str,
    query: SelectQuery<D, S, Row>,
) -> Cte<(), Row> {
    Cte::named(name, query)
}

/// A `LATERAL` subquery exposing its rows under the table marker `T`.
///
/// A lateral subquery may reference columns of the tables to its left, so it is
/// rendered inline in the join clause instead of as a named CTE. The marker `T`
/// names the alias and describes the produced columns, while `Row` records the
/// Rust type of the subquery output.
pub struct Lateral<T = (), Row = ()> {
    renderer: CteRenderer,
    _marker: PhantomData<fn() -> (T, Row)>,
}

impl<T, Row> fmt::Debug for Lateral<T, Row> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Lateral").finish()
    }
}

impl<T: Table, Row> Lateral<T, Row> {
    /// Wraps `query` into a `LATERAL` subquery exposed under the table `T`.
    pub fn new<D: Dialect, S: TableScope + 'static>(query: SelectQuery<D, S, Row>) -> Self
    where
        Row: 'static,
    {
        Self {
            renderer: Arc::new(move |sql, params| {
                query.render_into(sql, params);
            }),
            _marker: PhantomData,
        }
    }
}

/// Wraps `query` into a `LATERAL` subquery exposing its rows under the table marker `T`.
pub fn lateral<T: Table, D: Dialect, S: TableScope + 'static, Row: 'static>(
    query: SelectQuery<D, S, Row>,
) -> Lateral<T, Row> {
    Lateral::new(query)
}

/// A `SELECT` query over the table `Tables` producing rows of type `Output`.
///
/// `Tables` remains `()` until [`SelectQuery::from`] introduces the queried
/// table and `Output` remains `()` until [`SelectQuery::select`] fixes the row
/// type, which keeps the builder signature flat while tracking both the scope
/// and the result type at compile time.
pub struct SelectQuery<D: Dialect, Tables = (), Output = ()> {
    ctes: Vec<CteDefinition>,
    projections: Option<ProjectionRenderer>,
    joins: Vec<JoinClause>,
    filter: Option<ExprKind>,
    group_by: Vec<&'static str>,
    having: Option<ExprKind>,
    order_by: Vec<(&'static str, OrderDirection)>,
    limit: Option<u32>,
    offset: Option<u32>,
    set_operations: Vec<SetOperation>,
    _marker: QueryMarker<D, Tables, Output>,
}

impl<D: Dialect, Tables, Output> Clone for SelectQuery<D, Tables, Output> {
    fn clone(&self) -> Self {
        Self {
            ctes: self.ctes.clone(),
            projections: self.projections.clone(),
            joins: self.joins.clone(),
            filter: self.filter.clone(),
            group_by: self.group_by.clone(),
            having: self.having.clone(),
            order_by: self.order_by.clone(),
            limit: self.limit,
            offset: self.offset,
            set_operations: self.set_operations.clone(),
            _marker: PhantomData,
        }
    }
}

/// Phantom marker carrying the dialect, scope, and row type of a query.
///
/// The parameters are wrapped in a function result, so they are treated as
/// covariant and no drop-check obligations are introduced.
type QueryMarker<D, Tables, Output> = PhantomData<fn() -> (D, Tables, Output)>;

type ProjectionRenderer = Arc<dyn Fn(&mut String, &mut Vec<SqlVal>) + Send + Sync>;

/// The flavour of a relational set operation combining two queries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SetOpKind {
    /// `UNION` combines the rows of both queries and removes duplicates.
    Union,
    /// `UNION ALL` combines the rows of both queries and keeps duplicates.
    UnionAll,
    /// `INTERSECT` keeps only the rows produced by both queries.
    Intersect,
    /// `EXCEPT` keeps only the rows of the left query that the right one lacks.
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

/// Internal record of a set operation chained onto a [`SelectQuery`].
#[derive(Clone)]
struct SetOperation {
    kind: SetOpKind,
    render: ProjectionRenderer,
}

impl<D: Dialect> SelectQuery<D, (), ()> {
    /// Creates a query without a table, projections, filter, or pagination.
    pub fn new() -> Self {
        SelectQuery {
            ctes: Vec::new(),
            projections: None,
            joins: Vec::new(),
            filter: None,
            group_by: Vec::new(),
            having: None,
            order_by: Vec::new(),
            limit: None,
            offset: None,
            set_operations: Vec::new(),
            _marker: PhantomData,
        }
    }
}

impl<D: Dialect> Default for SelectQuery<D, (), ()> {
    fn default() -> Self {
        Self::new()
    }
}

impl<D: Dialect, Output> SelectQuery<D, (), Output> {
    /// Introduces `T` as the table rows are selected from.
    pub fn from<T: Table>(self) -> SelectQuery<D, T, Output> {
        self.retype()
    }
}

impl<D: Dialect, Tables> SelectQuery<D, Tables, ()> {
    /// Selects `selection`, fixing the row type of the query to [`Selection::Row`].
    pub fn select<S: Selection + Send + Sync + 'static>(
        mut self,
        selection: S,
    ) -> SelectQuery<D, Tables, S::Row> {
        self.projections = Some(Arc::new(move |sql, params| {
            selection.render_projections::<D>(sql, params);
        }));
        self.retype()
    }
}

impl<D: Dialect, Tables, Output> SelectQuery<D, Tables, Output> {
    /// Adds a Common Table Expression (`WITH cte AS (...)`) to this query.
    pub fn with<T, Row>(mut self, cte: Cte<T, Row>) -> Self {
        self.ctes.push(CteDefinition {
            name: cte.name,
            renderer: cte.renderer,
        });
        self
    }

    /// Converts this query into a CTE for the table `T`.
    pub fn as_cte<T: Table>(self) -> Cte<T, Output>
    where
        Tables: TableScope + 'static,
        Output: 'static,
    {
        Cte::new(self)
    }

    /// Converts this query into a named CTE with alias `name`.
    pub fn as_named_cte(self, name: &'static str) -> Cte<(), Output>
    where
        Tables: TableScope + 'static,
        Output: 'static,
    {
        Cte::named(name, self)
    }

    /// Converts this query into a `LATERAL` subquery exposed under the table marker `T`.
    pub fn as_lateral<T: Table>(self) -> Lateral<T, Output>
    where
        Tables: TableScope + 'static,
        Output: 'static,
    {
        Lateral::new(self)
    }

    /// Lifts the column `C` into its nullable form [`Nullable`].
    ///
    /// Columns of a table introduced with
    /// [`left_join`](SelectQuery::left_join) may be missing from a result row, so
    /// projecting them through `nullable` yields `Option<C::RustType>` instead of
    /// `C::RustType`.
    pub fn nullable<C: Column>(&self) -> Nullable<C> {
        Nullable::new()
    }

    /// Restricts the query to rows matching `predicate`.
    ///
    /// Repeated calls narrow the result with `AND`, so each predicate may be
    /// stated separately at its own call site.
    pub fn where_(mut self, predicate: Expr<bool>) -> Self {
        let predicate = predicate.into_kind();
        let filter = match self.filter.take() {
            Some(existing) => ExprKind::Binary {
                op: BinaryOp::And,
                lhs: Box::new(existing),
                rhs: Box::new(predicate),
            },
            None => predicate,
        };
        self.filter = Some(filter);
        self
    }

    /// Groups query results by `columns`.
    ///
    /// Calling this multiple times appends further grouping columns.
    pub fn group_by<G: GroupBy>(mut self, columns: G) -> Self {
        columns.push_column_names(&mut self.group_by);
        self
    }

    /// Restricts grouped rows to those satisfying `predicate`.
    ///
    /// Repeated calls combine conditions with SQL `AND`.
    pub fn having(mut self, predicate: Expr<bool>) -> Self {
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

    /// Appends an ordering criterion for column `col` with `direction`.
    pub fn order_by<C: Column>(mut self, _col: C, direction: OrderDirection) -> Self {
        self.order_by.push((C::NAME, direction));
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

    /// Limits the number of returned rows to `limit`.
    pub fn limit(mut self, limit: u32) -> Self {
        self.limit = Some(limit);
        self
    }

    /// Skips the first `offset` rows of the result.
    pub fn offset(mut self, offset: u32) -> Self {
        self.offset = Some(offset);
        self
    }

    /// Joins the table `T` to the query with an `INNER JOIN` restricted by `on`.
    ///
    /// The joined table becomes part of the query scope, so several joins may be
    /// chained. Parameters of `on` are bound before the parameters of the
    /// `WHERE` clause.
    pub fn inner_join<T: Table>(
        mut self,
        on: Expr<bool>,
    ) -> SelectQuery<D, Joined<Tables, T>, Output>
    where
        Tables: TableScope,
    {
        self.joins.push(JoinClause {
            kind: JoinKind::Inner,
            on: Some(on.into_kind()),
            lateral: None,
        });
        self.retype()
    }

    /// Joins the table `T` to the query with a `LEFT JOIN` restricted by `on`.
    ///
    /// Rows of `T` may be missing from the result, so its columns are projected
    /// through [`SelectQuery::nullable`] to keep their absence visible in the row
    /// type. Parameters of `on` are bound before the parameters of the `WHERE`
    /// clause.
    pub fn left_join<T: Table>(
        mut self,
        on: Expr<bool>,
    ) -> SelectQuery<D, LeftJoined<Tables, T>, Output>
    where
        Tables: TableScope,
    {
        self.joins.push(JoinClause {
            kind: JoinKind::Left,
            on: Some(on.into_kind()),
            lateral: None,
        });
        self.retype()
    }

    fn retype<Tables2, Output2>(self) -> SelectQuery<D, Tables2, Output2> {
        SelectQuery {
            ctes: self.ctes,
            projections: self.projections,
            joins: self.joins,
            filter: self.filter,
            group_by: self.group_by,
            having: self.having,
            order_by: self.order_by,
            limit: self.limit,
            offset: self.offset,
            set_operations: self.set_operations,
            _marker: PhantomData,
        }
    }
}

impl<D: Dialect + SupportsLateralJoin, Tables: TableScope, Output> SelectQuery<D, Tables, Output> {
    /// Joins `lateral` to the query with a `CROSS JOIN LATERAL`.
    ///
    /// The subquery is rendered inline and may reference columns of the tables
    /// to its left, so every outer row is joined with all rows the subquery
    /// produces for it. Parameters of the subquery are bound before the
    /// parameters of the `WHERE` clause.
    pub fn cross_join_lateral<T: Table, Row: 'static>(
        mut self,
        lateral: Lateral<T, Row>,
    ) -> SelectQuery<D, CrossLateralJoined<Tables, T>, Output> {
        self.joins.push(JoinClause {
            kind: JoinKind::CrossLateral,
            on: None,
            lateral: Some(lateral.renderer),
        });
        self.retype()
    }

    /// Joins `lateral` to the query with a `LEFT JOIN LATERAL ... ON true`.
    ///
    /// Unlike [`cross_join_lateral`](SelectQuery::cross_join_lateral), outer rows
    /// are kept even when the subquery produces no rows, so its columns are
    /// projected through [`SelectQuery::nullable`] to lift them into `Option<_>`
    /// at compile time.
    pub fn left_join_lateral<T: Table, Row: 'static>(
        mut self,
        lateral: Lateral<T, Row>,
    ) -> SelectQuery<D, LeftLateralJoined<Tables, T>, Output> {
        self.joins.push(JoinClause {
            kind: JoinKind::LeftLateral,
            on: Some(ExprKind::Raw {
                fragments: vec!["true".to_owned()],
                params: Vec::new(),
            }),
            lateral: Some(lateral.renderer),
        });
        self.retype()
    }
}

impl<D: Dialect, Tables, Output: 'static> SelectQuery<D, Tables, Output> {
    /// Combines this query with `other` using `UNION`, removing duplicate rows.
    ///
    /// Both queries must target the same dialect `D` and produce the same row
    /// type `Output`, so the combined result stays decodable as `Output`.
    pub fn union<Tables2: TableScope + 'static>(
        self,
        other: SelectQuery<D, Tables2, Output>,
    ) -> Self {
        self.with_set_operation(SetOpKind::Union, other)
    }

    /// Combines this query with `other` using `UNION ALL`, keeping duplicates.
    pub fn union_all<Tables2: TableScope + 'static>(
        self,
        other: SelectQuery<D, Tables2, Output>,
    ) -> Self {
        self.with_set_operation(SetOpKind::UnionAll, other)
    }

    /// Keeps only the rows produced by both this query and `other` (`INTERSECT`).
    pub fn intersect<Tables2: TableScope + 'static>(
        self,
        other: SelectQuery<D, Tables2, Output>,
    ) -> Self {
        self.with_set_operation(SetOpKind::Intersect, other)
    }

    /// Keeps the rows of this query that are absent from `other` (`EXCEPT`).
    pub fn except<Tables2: TableScope + 'static>(
        self,
        other: SelectQuery<D, Tables2, Output>,
    ) -> Self {
        self.with_set_operation(SetOpKind::Except, other)
    }

    fn with_set_operation<Tables2: TableScope + 'static>(
        mut self,
        kind: SetOpKind,
        other: SelectQuery<D, Tables2, Output>,
    ) -> Self {
        self.set_operations.push(SetOperation {
            kind,
            render: Arc::new(move |sql, params| other.render_into(sql, params)),
        });
        self
    }
}

impl<D: Dialect, Tables: TableScope, Output> SelectQuery<D, Tables, Output> {
    /// Renders the query statement and parameters into the target buffers.
    pub fn render_into(&self, sql: &mut String, params: &mut Vec<SqlVal>) {
        let render_projections = self.projections.as_ref().expect(
            "a SELECT query requires at least one projection; call `select` before `build`",
        );

        let mut tables = Vec::new();
        Tables::push_table_refs(&mut tables);
        assert_eq!(
            tables.len(),
            self.joins.len() + 1,
            "every joined table requires an ON condition"
        );

        if !self.ctes.is_empty() {
            sql.push_str("WITH ");
            for (index, cte) in self.ctes.iter().enumerate() {
                if index > 0 {
                    sql.push_str(", ");
                }
                D::push_quoted_identifier(sql, cte.name);
                sql.push_str(" AS (");
                (cte.renderer)(sql, params);
                sql.push(')');
            }
            sql.push(' ');
        }

        sql.push_str("SELECT ");
        render_projections(sql, params);

        sql.push_str(" FROM ");
        tables[0].render::<D>(sql);

        for (index, join) in self.joins.iter().enumerate() {
            sql.push(' ');
            sql.push_str(join.kind.as_str());
            sql.push(' ');
            match &join.lateral {
                Some(render_lateral) => {
                    sql.push('(');
                    render_lateral(sql, params);
                    sql.push_str(") AS ");
                    D::push_quoted_identifier(sql, tables[index + 1].name);
                }
                None => tables[index + 1].render::<D>(sql),
            }
            if let Some(on) = &join.on {
                sql.push_str(" ON ");
                render_expr::<D>(sql, params, on);
            }
        }

        if let Some(filter) = &self.filter {
            sql.push_str(" WHERE ");
            render_expr::<D>(sql, params, filter);
        }

        if !self.group_by.is_empty() {
            sql.push_str(" GROUP BY ");
            for (index, name) in self.group_by.iter().enumerate() {
                if index > 0 {
                    sql.push_str(", ");
                }
                D::push_quoted_identifier(sql, name);
            }
        }

        if let Some(having) = &self.having {
            sql.push_str(" HAVING ");
            render_expr::<D>(sql, params, having);
        }

        for operation in &self.set_operations {
            sql.push(' ');
            sql.push_str(operation.kind.as_str());
            sql.push(' ');
            (operation.render)(sql, params);
        }

        if !self.order_by.is_empty() {
            sql.push_str(" ORDER BY ");
            for (index, (name, direction)) in self.order_by.iter().enumerate() {
                if index > 0 {
                    sql.push_str(", ");
                }
                D::push_quoted_identifier(sql, name);
                sql.push(' ');
                sql.push_str(direction.as_str());
            }
        }

        if let Some(limit) = self.limit {
            let _ = write!(sql, " LIMIT {limit}");
        }
        if let Some(offset) = self.offset {
            let _ = write!(sql, " OFFSET {offset}");
        }
    }

    /// Renders the query into SQL text and its ordered parameter list.
    pub fn build(&self) -> BuiltQuery {
        let mut sql = String::new();
        let mut params = Vec::new();
        self.render_into(&mut sql, &mut params);
        BuiltQuery { sql, params }
    }
}

impl<D: Dialect, Tables: TableScope + 'static, Output: 'static> SelectQuery<D, Tables, Output> {
    /// Wraps the query into a [`Subquery`] rendered with the enclosing statement's parameters.
    pub fn into_subquery(self) -> Subquery {
        Subquery::new(move |sql, params| self.render_into(sql, params))
    }

    /// Builds an `EXISTS (subquery)` predicate over this query.
    pub fn exists(&self) -> Expr<bool> {
        Expr::exists(self.clone())
    }

    /// Builds a `NOT EXISTS (subquery)` predicate over this query.
    pub fn not_exists(&self) -> Expr<bool> {
        Expr::not_exists(self.clone())
    }
}

impl<D: Dialect, Tables: TableScope + 'static, Output: 'static> From<SelectQuery<D, Tables, Output>>
    for Subquery
{
    fn from(query: SelectQuery<D, Tables, Output>) -> Self {
        query.into_subquery()
    }
}

use crate::expr::render_expr;

/// A query whose statement can be rendered and whose rows decode into `Row`.
///
/// The trait gives every query builder — [`SelectQuery`], [`InsertQuery`],
/// [`UpdateQuery`], and [`DeleteQuery`] — a uniform view on the rendered
/// [`BuiltQuery`] and on the Rust type produced for a single result row, so
/// execution helpers accept any of them. Queries without a `RETURNING` clause
/// expose `()` as their row type, reflecting that they produce no rows.
#[diagnostic::on_unimplemented(
    message = "`{Self}` cannot be executed as a query",
    label = "unsupported query type",
    note = "executable queries are `SelectQuery`, `InsertQuery`, `UpdateQuery`, and `DeleteQuery`"
)]
pub trait ExecutableQuery {
    /// The Rust type produced for a single row of the query.
    type Row;

    /// Renders the statement into SQL text and its ordered parameter list.
    fn build_query(&self) -> BuiltQuery;
}

impl<D: Dialect, Tables: TableScope, Output> ExecutableQuery for SelectQuery<D, Tables, Output> {
    type Row = Output;

    fn build_query(&self) -> BuiltQuery {
        self.build()
    }
}

impl<D: Dialect, T: Table, Values, Returning> ExecutableQuery
    for InsertQuery<D, T, Values, Returning>
{
    type Row = Returning;

    fn build_query(&self) -> BuiltQuery {
        self.build()
    }
}

impl<D: Dialect, T: Table, Returning> ExecutableQuery for UpdateQuery<D, T, Filtered, Returning> {
    type Row = Returning;

    fn build_query(&self) -> BuiltQuery {
        self.build()
    }
}

impl<D: Dialect, T: Table, Returning> ExecutableQuery for DeleteQuery<D, T, Filtered, Returning> {
    type Row = Returning;

    fn build_query(&self) -> BuiltQuery {
        self.build()
    }
}

/// A database driver capable of executing rendered queries.
///
/// A backend receives the statement as a [`BuiltQuery`], so it binds the typed
/// [`SqlVal`] parameters through its native driver API instead of interpolating
/// SQL text. Rows come back as ordered value lists, which
/// [`QueryBackendExt`] decodes through [`FromRow`].
#[diagnostic::on_unimplemented(
    message = "`{Self}` cannot execute RURQE queries",
    label = "unsupported query backend",
    note = "driver adapters implement `QueryBackend` by translating `SqlVal` parameters into native driver values"
)]
pub trait QueryBackend {
    /// The error raised by the driver.
    type Error;

    /// Executes `query` and returns every produced row as ordered values.
    fn fetch_raw(&mut self, query: &BuiltQuery) -> Result<Vec<Vec<SqlVal>>, Self::Error>;

    /// Executes `query` without decoding rows and returns the affected row count.
    fn execute_raw(&mut self, query: &BuiltQuery) -> Result<u64, Self::Error>;
}

/// Error raised while executing a query through [`QueryBackendExt`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExecutionError<E> {
    /// The driver failed to execute the statement.
    Backend(E),
    /// A produced row did not match the row type of the query.
    RowDecode {
        /// The zero-based index of the first row that failed to decode.
        row_index: usize,
    },
    /// A single-row query produced no rows.
    NoRows,
    /// A single-row query produced more than one row.
    MultipleRows {
        /// The number of rows the query produced.
        count: usize,
    },
}

impl<E: fmt::Display> fmt::Display for ExecutionError<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ExecutionError::Backend(error) => write!(f, "database backend error: {error}"),
            ExecutionError::RowDecode { row_index } => write!(
                f,
                "failed to decode row {row_index}: values do not match the query row type"
            ),
            ExecutionError::NoRows => {
                f.write_str("expected exactly one row, but the query returned none")
            }
            ExecutionError::MultipleRows { count } => {
                write!(f, "expected at most one row, but the query returned {count}")
            }
        }
    }
}

impl<E: std::error::Error + 'static> std::error::Error for ExecutionError<E> {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ExecutionError::Backend(error) => Some(error),
            ExecutionError::RowDecode { .. }
            | ExecutionError::NoRows
            | ExecutionError::MultipleRows { .. } => None,
        }
    }
}

/// Execution helpers decoding query rows through [`FromRow`].
///
/// The trait is implemented for every [`QueryBackend`], adding typed helpers on
/// top of the raw driver contract:
///
/// - [`fetch_all`](QueryBackendExt::fetch_all) decodes every produced row.
/// - [`fetch_optional`](QueryBackendExt::fetch_optional) decodes at most one row.
/// - [`fetch_one`](QueryBackendExt::fetch_one) requires exactly one row.
/// - [`execute`](QueryBackendExt::execute) returns the affected row count.
#[diagnostic::on_unimplemented(
    message = "`{Self}` is not a RURQE query backend",
    label = "missing backend implementation",
    note = "implement `QueryBackend` for the driver; `QueryBackendExt` is then available for free"
)]
pub trait QueryBackendExt: QueryBackend {
    /// Executes `query` and decodes every produced row.
    fn fetch_all<Q>(&mut self, query: &Q) -> Result<Vec<Q::Row>, ExecutionError<Self::Error>>
    where
        Q: ExecutableQuery,
        Q::Row: FromRow,
    {
        let built = query.build_query();
        let rows = self.fetch_raw(&built).map_err(ExecutionError::Backend)?;
        rows.iter()
            .enumerate()
            .map(|(row_index, values)| {
                Q::Row::from_row(values).ok_or(ExecutionError::RowDecode { row_index })
            })
            .collect()
    }

    /// Executes `query` and decodes at most one row.
    ///
    /// Returns `None` when the query produces no rows and
    /// [`ExecutionError::MultipleRows`] when it produces more than one.
    fn fetch_optional<Q>(
        &mut self,
        query: &Q,
    ) -> Result<Option<Q::Row>, ExecutionError<Self::Error>>
    where
        Q: ExecutableQuery,
        Q::Row: FromRow,
    {
        let rows = self.fetch_all(query)?;
        match rows.len() {
            0 => Ok(None),
            1 => Ok(rows.into_iter().next()),
            count => Err(ExecutionError::MultipleRows { count }),
        }
    }

    /// Executes `query` and decodes exactly one row.
    ///
    /// Returns [`ExecutionError::NoRows`] when the query produces no rows and
    /// [`ExecutionError::MultipleRows`] when it produces more than one.
    fn fetch_one<Q>(&mut self, query: &Q) -> Result<Q::Row, ExecutionError<Self::Error>>
    where
        Q: ExecutableQuery,
        Q::Row: FromRow,
    {
        self.fetch_optional(query)?.ok_or(ExecutionError::NoRows)
    }

    /// Executes `query` and returns the number of affected rows.
    fn execute<Q>(&mut self, query: &Q) -> Result<u64, ExecutionError<Self::Error>>
    where
        Q: ExecutableQuery,
    {
        let built = query.build_query();
        self.execute_raw(&built).map_err(ExecutionError::Backend)
    }
}

impl<B: QueryBackend + ?Sized> QueryBackendExt for B {}

/// The isolation level a transaction runs under.
///
/// The level is rendered into the `ISOLATION LEVEL` clause of the statement
/// issued by [`TransactionBackend::begin`], letting the engine trade
/// concurrency for consistency (for example, `SERIALIZABLE` behaves as if the
/// conflicting transactions ran one after another).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IsolationLevel {
    /// Reads only rows committed before the statement started.
    ReadCommitted,
    /// Reads a stable snapshot, so repeated reads observe the same rows.
    RepeatableRead,
    /// Behaves as if transactions ran one after another.
    Serializable,
}

impl IsolationLevel {
    /// The SQL text of the corresponding `ISOLATION LEVEL` clause.
    pub const fn as_str(self) -> &'static str {
        match self {
            IsolationLevel::ReadCommitted => "READ COMMITTED",
            IsolationLevel::RepeatableRead => "REPEATABLE READ",
            IsolationLevel::Serializable => "SERIALIZABLE",
        }
    }
}

/// Tracks the nesting depth and savepoint names of an open transaction.
///
/// The outermost transaction is committed or rolled back as a whole, while each
/// nested block runs inside a savepoint. The context allocates unique savepoint
/// names and remembers the active nesting, so a [`TransactionBackend`] can turn
/// a nested transaction into `SAVEPOINT`, `RELEASE SAVEPOINT`, and
/// `ROLLBACK TO SAVEPOINT` statements without the caller tracking depth.
#[derive(Debug, Clone, Default)]
pub struct TransactionContext {
    next_savepoint: u32,
    savepoints: Vec<String>,
}

impl TransactionContext {
    /// Creates a context for the outermost transaction.
    pub fn new() -> Self {
        TransactionContext::default()
    }

    /// The number of nested savepoints currently open.
    pub fn depth(&self) -> usize {
        self.savepoints.len()
    }

    /// Whether the context currently sits inside a nested transaction.
    pub fn is_nested(&self) -> bool {
        !self.savepoints.is_empty()
    }

    /// The name of the innermost open savepoint, if any.
    pub fn current_savepoint(&self) -> Option<&str> {
        self.savepoints.last().map(String::as_str)
    }

    /// Allocates a fresh savepoint name and makes it the innermost block.
    pub fn open_savepoint(&mut self) -> String {
        self.next_savepoint += 1;
        let name = format!("rurqe_sp_{}", self.next_savepoint);
        self.savepoints.push(name.clone());
        name
    }

    /// Ends the innermost block and returns the savepoint name it used.
    pub fn close_savepoint(&mut self) -> Option<String> {
        self.savepoints.pop()
    }
}

/// A column or tuple of columns for the `PARTITION BY` clause of a window specification.
#[diagnostic::on_unimplemented(
    message = "`{Self}` is not a valid PARTITION BY clause",
    label = "partition by a column or a tuple of columns",
    note = "partition by accepts column markers or tuples of column markers, such as `UserId` or `(UserId, UserName)`"
)]
pub trait PartitionBy {
    /// Appends the partition column names to `out`.
    fn push_partition_by(&self, out: &mut Vec<&'static str>);
}

impl<C: Column> PartitionBy for C {
    fn push_partition_by(&self, out: &mut Vec<&'static str>) {
        out.push(C::NAME);
    }
}

macro_rules! impl_partition_by {
    ($($column:ident),+) => {
        impl<$($column: Column),+> PartitionBy for ($($column,)+) {
            fn push_partition_by(&self, out: &mut Vec<&'static str>) {
                $(out.push($column::NAME);)+
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
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WindowSpec {
    partition_by: Vec<&'static str>,
    order_by: Vec<(&'static str, OrderDirection)>,
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
        self.order_by.push((C::NAME, direction));
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
    pub fn render<D: Dialect>(&self, sql: &mut String) {
        let mut has_part = false;
        if !self.partition_by.is_empty() {
            has_part = true;
            sql.push_str("PARTITION BY ");
            for (i, name) in self.partition_by.iter().enumerate() {
                if i > 0 {
                    sql.push_str(", ");
                }
                D::push_quoted_identifier(sql, name);
            }
        }
        if !self.order_by.is_empty() {
            if has_part {
                sql.push(' ');
            }
            sql.push_str("ORDER BY ");
            for (i, (name, dir)) in self.order_by.iter().enumerate() {
                if i > 0 {
                    sql.push_str(", ");
                }
                D::push_quoted_identifier(sql, name);
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
        /// The target column name.
        column: &'static str,
        /// The optional row offset.
        offset: Option<u32>,
    },
    /// `LEAD(column [, offset])`
    Lead {
        /// The target column name.
        column: &'static str,
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
                D::push_quoted_identifier(sql, column);
                if let Some(off) = offset {
                    let _ = write!(sql, ", {off}");
                }
                sql.push(')');
            }
            WindowExprKind::Lead { column, offset } => {
                sql.push_str("LEAD(");
                D::push_quoted_identifier(sql, column);
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
        self.window.render::<D>(sql);
        sql.push(')');
    }
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
    column: &'static str,
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
        column: C::NAME,
        offset: None,
        _result: PhantomData,
    }
}

/// Starts a `LEAD(C)` window function builder for column `C`.
pub fn lead<C: Column>(_col: C) -> LagLeadFn<Option<C::RustType>> {
    LagLeadFn {
        is_lag: false,
        column: C::NAME,
        offset: None,
        _result: PhantomData,
    }
}

/// Runs a parameterless statement through a [`QueryBackend`].
fn execute_statement<B: QueryBackend + ?Sized>(
    backend: &mut B,
    sql: String,
) -> Result<(), B::Error> {
    backend
        .execute_raw(&BuiltQuery {
            sql,
            params: Vec::new(),
        })
        .map(|_| ())
}

/// A synchronous connection able to run transactions and nested savepoints.
///
/// The trait is the driver-side half of RAII-managed transactions: it exposes
/// the primitives — `BEGIN`, `COMMIT`, `ROLLBACK`, and the savepoint statements
/// — that the transaction executor drives to commit on `Ok(_)` and roll back on
/// `Err(_)` or panic. Because a nested transaction is a savepoint, a failing
/// inner block only rolls back to its savepoint and leaves the outer transaction
/// intact.
///
/// Every method has a dialect-neutral default implementation that renders the
/// statement and forwards it to [`QueryBackend::execute_raw`], so a driver that
/// already implements [`QueryBackend`] gains transaction support for free and
/// only overrides the statements its engine spells differently.
#[diagnostic::on_unimplemented(
    message = "`{Self}` cannot run RURQE transactions",
    label = "unsupported transaction backend",
    note = "synchronous connections implement `TransactionBackend` on top of `QueryBackend`"
)]
pub trait TransactionBackend: QueryBackend {
    /// Opens the outermost transaction with the given isolation level.
    ///
    /// Passing `None` keeps the engine default isolation level.
    fn begin(&mut self, isolation: Option<IsolationLevel>) -> Result<(), Self::Error> {
        let mut sql = String::from("BEGIN");
        if let Some(level) = isolation {
            sql.push_str(" ISOLATION LEVEL ");
            sql.push_str(level.as_str());
        }
        execute_statement(self, sql)
    }

    /// Commits the current transaction, persisting all its work.
    fn commit(&mut self) -> Result<(), Self::Error> {
        execute_statement(self, String::from("COMMIT"))
    }

    /// Rolls the current transaction back, discarding all its work.
    fn rollback(&mut self) -> Result<(), Self::Error> {
        execute_statement(self, String::from("ROLLBACK"))
    }

    /// Opens the nested transaction `name` as a savepoint.
    fn savepoint(&mut self, name: &str) -> Result<(), Self::Error> {
        execute_statement(self, format!("SAVEPOINT {name}"))
    }

    /// Releases the savepoint `name`, keeping its work in the transaction.
    fn release_savepoint(&mut self, name: &str) -> Result<(), Self::Error> {
        execute_statement(self, format!("RELEASE SAVEPOINT {name}"))
    }

    /// Rolls the transaction back to the savepoint `name`, discarding only the
    /// work done inside the nested block.
    fn rollback_to_savepoint(&mut self, name: &str) -> Result<(), Self::Error> {
        execute_statement(self, format!("ROLLBACK TO SAVEPOINT {name}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dml::ColumnAssign;
    use crate::schema::{Aliased, AliasedCol};

    struct MockDialect;

    impl Dialect for MockDialect {
        fn push_quoted_identifier(out: &mut String, ident: &str) {
            out.push('"');
            out.push_str(ident);
            out.push('"');
        }

        fn push_placeholder(out: &mut String, index: usize) {
            let _ = write!(out, "${index}");
        }
    }

    impl SupportsLateralJoin for MockDialect {}

    #[derive(Default)]
    struct MockBackend {
        rows: Vec<Vec<SqlVal>>,
        affected: u64,
        executed: Vec<BuiltQuery>,
    }

    impl QueryBackend for MockBackend {
        type Error = &'static str;

        fn fetch_raw(&mut self, query: &BuiltQuery) -> Result<Vec<Vec<SqlVal>>, Self::Error> {
            self.executed.push(query.clone());
            Ok(self.rows.clone())
        }

        fn execute_raw(&mut self, query: &BuiltQuery) -> Result<u64, Self::Error> {
            self.executed.push(query.clone());
            Ok(self.affected)
        }
    }

    crate::table! {
        User => "users" {
            UserId => "id": i64,
            UserName => "name": String,
        }
    }

    crate::table! {
        Order => "orders" {
            OrderId => "id": i64,
            OrderUserId => "user_id": i64,
            OrderTotal => "total": f64,
        }
    }

    #[test]
    fn renders_selection_from_filter_and_pagination() {
        let built = SelectQuery::<MockDialect>::new()
            .from::<User>()
            .select((UserId, UserName))
            .where_(Expr::column::<UserId>().gt(10i64))
            .limit(20)
            .offset(5)
            .build();

        assert_eq!(
            built.sql,
            "SELECT \"id\", \"name\" FROM \"users\" WHERE (\"id\" > $1) LIMIT 20 OFFSET 5"
        );
        assert_eq!(built.params, vec![SqlVal::I64(10)]);
    }

    #[test]
    fn selection_determines_row_type() {
        let query: SelectQuery<MockDialect, User, (i64,)> =
            SelectQuery::<MockDialect>::new().from::<User>().select((UserId,));

        assert_eq!(query.build().sql, "SELECT \"id\" FROM \"users\"");
    }

    #[test]
    fn projections_and_table_may_be_declared_in_any_order() {
        let built = SelectQuery::<MockDialect>::new()
            .select((UserId,))
            .from::<User>()
            .build();

        assert_eq!(built.sql, "SELECT \"id\" FROM \"users\"");
    }

    #[test]
    fn repeated_predicates_are_combined_with_and() {
        let built = SelectQuery::<MockDialect>::new()
            .from::<User>()
            .select((UserId, UserName))
            .where_(Expr::column::<UserId>().gt(1i64))
            .where_(Expr::column::<UserName>().eq("root"))
            .build();

        assert_eq!(
            built.sql,
            "SELECT \"id\", \"name\" FROM \"users\" WHERE ((\"id\" > $1) AND (\"name\" = $2))"
        );
        assert_eq!(
            built.params,
            vec![SqlVal::I64(1), SqlVal::Text("root".to_owned())]
        );
    }

    #[test]
    fn order_by_renders_asc_and_desc() {
        let built = SelectQuery::<MockDialect>::new()
            .from::<User>()
            .select((UserId, UserName))
            .order_by(UserId, OrderDirection::Desc)
            .order_by_asc(UserName)
            .build();

        assert_eq!(
            built.sql,
            "SELECT \"id\", \"name\" FROM \"users\" ORDER BY \"id\" DESC, \"name\" ASC"
        );
    }

    #[test]
    #[should_panic(expected = "at least one projection")]
    fn building_without_projections_panics() {
        let _ = SelectQuery::<MockDialect>::new().from::<User>().build();
    }

    #[test]
    fn inner_join_renders_join_clause_before_filter() {
        let query: SelectQuery<MockDialect, Joined<User, Order>, (i64, i64)> =
            SelectQuery::<MockDialect>::new()
                .from::<User>()
                .inner_join::<Order>(Expr::column::<OrderUserId>().eq(7i64))
                .select((UserId, OrderId));

        let built = query.where_(Expr::column::<UserId>().gt(1i64)).build();

        assert_eq!(
            built.sql,
            "SELECT \"id\", \"id\" FROM \"users\" INNER JOIN \"orders\" \
             ON (\"user_id\" = $1) WHERE (\"id\" > $2)"
        );
        assert_eq!(built.params, vec![SqlVal::I64(7), SqlVal::I64(1)]);
    }

    #[test]
    fn joined_scope_tracks_every_table_in_from_order() {
        let built = SelectQuery::<MockDialect>::new()
            .from::<User>()
            .inner_join::<Order>(Expr::column::<OrderUserId>().eq(1i64))
            .inner_join::<User>(Expr::column::<UserId>().eq(2i64))
            .select((UserId,))
            .build();

        assert_eq!(
            built.sql,
            "SELECT \"id\" FROM \"users\" INNER JOIN \"orders\" ON (\"user_id\" = $1) \
             INNER JOIN \"users\" ON (\"id\" = $2)"
        );
        assert_eq!(built.params, vec![SqlVal::I64(1), SqlVal::I64(2)]);
    }

    #[test]
    fn single_table_and_joined_scopes_are_query_scopes() {
        fn assert_scope<S: TableScope>() {}

        assert_scope::<User>();
        assert_scope::<Joined<User, Order>>();
        assert_scope::<LeftJoined<User, Order>>();
        assert_scope::<Joined<Joined<User, Order>, User>>();
        assert_scope::<Joined<LeftJoined<User, Order>, User>>();
    }

    #[test]
    fn left_join_renders_left_keyword_and_composes_with_inner_join() {
        let query = SelectQuery::<MockDialect>::new()
            .from::<User>()
            .left_join::<Order>(Expr::column::<OrderUserId>().eq(7i64))
            .inner_join::<User>(Expr::column::<UserId>().eq(2i64));
        let total = query.nullable::<OrderTotal>();

        let built = query.select((UserId, total)).build();

        assert_eq!(
            built.sql,
            "SELECT \"id\", \"total\" FROM \"users\" LEFT JOIN \"orders\" ON (\"user_id\" = $1) \
             INNER JOIN \"users\" ON (\"id\" = $2)"
        );
        assert_eq!(built.params, vec![SqlVal::I64(7), SqlVal::I64(2)]);
    }

    #[test]
    fn nullable_column_keeps_name_and_lifts_rust_type() {
        fn takes_optional_total<C>()
        where
            C: Column<RustType = Option<f64>, Table = Order>,
        {
        }

        assert_eq!(Nullable::<OrderTotal>::NAME, OrderTotal::NAME);
        takes_optional_total::<Nullable<OrderTotal>>();
    }

    #[test]
    fn group_by_renders_single_column_and_tuple() {
        let single = SelectQuery::<MockDialect>::new()
            .from::<User>()
            .select((UserId,))
            .group_by(UserId)
            .build();
        assert_eq!(single.sql, "SELECT \"id\" FROM \"users\" GROUP BY \"id\"");

        let tuple = SelectQuery::<MockDialect>::new()
            .from::<User>()
            .select((UserId, UserName))
            .group_by((UserId, UserName))
            .build();
        assert_eq!(
            tuple.sql,
            "SELECT \"id\", \"name\" FROM \"users\" GROUP BY \"id\", \"name\""
        );
    }

    #[test]
    fn multiple_group_by_calls_append_columns() {
        let built = SelectQuery::<MockDialect>::new()
            .from::<User>()
            .select((UserId, UserName))
            .group_by(UserId)
            .group_by(UserName)
            .build();

        assert_eq!(
            built.sql,
            "SELECT \"id\", \"name\" FROM \"users\" GROUP BY \"id\", \"name\""
        );
    }

    #[test]
    fn having_renders_aggregate_filters_and_binds_parameters() {
        let built = SelectQuery::<MockDialect>::new()
            .from::<Order>()
            .select((OrderUserId,))
            .group_by(OrderUserId)
            .having(Expr::count_all().gt(5i64))
            .build();

        assert_eq!(
            built.sql,
            "SELECT \"user_id\" FROM \"orders\" GROUP BY \"user_id\" HAVING (COUNT(*) > $1)"
        );
        assert_eq!(built.params, vec![SqlVal::I64(5)]);
    }

    #[test]
    fn repeated_having_calls_are_combined_with_and() {
        let built = SelectQuery::<MockDialect>::new()
            .from::<Order>()
            .select((OrderUserId,))
            .where_(Expr::column::<OrderId>().gt(0i64))
            .group_by(OrderUserId)
            .having(Expr::count_all().gt(2i64))
            .having(Expr::sum::<OrderTotal>().lt(100.0))
            .order_by_asc(OrderUserId)
            .build();

        assert_eq!(
            built.sql,
            "SELECT \"user_id\" FROM \"orders\" WHERE (\"id\" > $1) GROUP BY \"user_id\" \
             HAVING ((COUNT(*) > $2) AND (SUM(\"total\") < $3)) ORDER BY \"user_id\" ASC"
        );
        assert_eq!(
            built.params,
            vec![SqlVal::I64(0), SqlVal::I64(2), SqlVal::F64(100.0)]
        );
    }

    #[test]
    fn left_joined_rows_decode_missing_columns_as_none() {
        use crate::row::FromRow;

        assert_eq!(
            <(i64, Option<f64>)>::from_row(&[SqlVal::I64(1), SqlVal::Null]),
            Some((1, None))
        );
    }

    #[test]
    fn fetch_all_renders_the_query_and_decodes_rows() {
        let query = SelectQuery::<MockDialect>::new()
            .from::<User>()
            .select((UserId, UserName))
            .where_(Expr::column::<UserId>().gt(10i64));

        let mut backend = MockBackend {
            rows: vec![
                vec![SqlVal::I64(11), SqlVal::Text("root".to_owned())],
                vec![SqlVal::I64(12), SqlVal::Text("guest".to_owned())],
            ],
            ..MockBackend::default()
        };

        let rows = backend.fetch_all(&query).expect("rows decode");

        assert_eq!(
            rows,
            vec![(11i64, "root".to_owned()), (12i64, "guest".to_owned())]
        );
        assert_eq!(backend.executed, vec![query.build()]);
    }

    #[test]
    fn fetch_one_reports_missing_and_duplicate_rows() {
        let query = SelectQuery::<MockDialect>::new()
            .from::<User>()
            .select((UserId,));

        let mut empty = MockBackend::default();
        assert_eq!(empty.fetch_one(&query), Err(ExecutionError::NoRows));

        let mut many = MockBackend {
            rows: vec![vec![SqlVal::I64(1)], vec![SqlVal::I64(2)]],
            ..MockBackend::default()
        };
        assert_eq!(
            many.fetch_one(&query),
            Err(ExecutionError::MultipleRows { count: 2 })
        );

        let mut single = MockBackend {
            rows: vec![vec![SqlVal::I64(7)]],
            ..MockBackend::default()
        };
        assert_eq!(single.fetch_one(&query), Ok((7i64,)));
    }

    #[test]
    fn fetch_optional_returns_none_for_absent_rows() {
        let query = SelectQuery::<MockDialect>::new()
            .from::<User>()
            .select((UserId,));

        let mut backend = MockBackend::default();
        assert_eq!(backend.fetch_optional(&query), Ok(None));

        backend.rows = vec![vec![SqlVal::I64(3)]];
        assert_eq!(backend.fetch_optional(&query), Ok(Some((3i64,))));
    }

    #[test]
    fn fetch_all_reports_the_row_that_fails_to_decode() {
        let query = SelectQuery::<MockDialect>::new()
            .from::<User>()
            .select((UserId,));

        let mut backend = MockBackend {
            rows: vec![vec![SqlVal::Text("oops".to_owned())]],
            ..MockBackend::default()
        };

        assert_eq!(
            backend.fetch_all(&query),
            Err(ExecutionError::RowDecode { row_index: 0 })
        );
    }

    #[test]
    fn execute_reports_affected_rows_without_decoding() {
        let query = InsertQuery::<MockDialect, User>::new()
            .values((UserId::set(1i64), UserName::set("root")));

        let mut backend = MockBackend {
            affected: 1,
            ..MockBackend::default()
        };

        assert_eq!(backend.execute(&query), Ok(1));
        assert_eq!(backend.executed, vec![query.build_query()]);
    }

    #[test]
    fn executable_queries_expose_built_sql_and_row_type() {
        let insert = InsertQuery::<MockDialect, User>::new()
            .values((UserId::set(1i64), UserName::set("root")));
        assert_eq!(insert.build_query(), insert.build());

        fn assert_row<Q: ExecutableQuery<Row = (i64, String)>>() {}

        assert_row::<SelectQuery<MockDialect, User, (i64, String)>>();
        assert_row::<InsertQuery<MockDialect, User, (), (i64, String)>>();
        assert_row::<UpdateQuery<MockDialect, User, Filtered, (i64, String)>>();
        assert_row::<DeleteQuery<MockDialect, User, Filtered, (i64, String)>>();
    }

    #[test]
    fn execution_errors_render_actionable_messages() {
        assert_eq!(
            ExecutionError::<&str>::Backend("connection lost").to_string(),
            "database backend error: connection lost"
        );
        assert_eq!(
            ExecutionError::<&str>::RowDecode { row_index: 2 }.to_string(),
            "failed to decode row 2: values do not match the query row type"
        );
        assert_eq!(
            ExecutionError::<&str>::NoRows.to_string(),
            "expected exactly one row, but the query returned none"
        );
        assert_eq!(
            ExecutionError::<&str>::MultipleRows { count: 3 }.to_string(),
            "expected at most one row, but the query returned 3"
        );
    }

    #[test]
    fn execution_error_implements_the_error_trait() {
        fn assert_error<E: std::error::Error>() {}

        assert_error::<ExecutionError<std::io::Error>>();
    }

    #[derive(Default)]
    struct MockTransactionBackend {
        statements: Vec<String>,
    }

    impl QueryBackend for MockTransactionBackend {
        type Error = &'static str;

        fn fetch_raw(&mut self, _query: &BuiltQuery) -> Result<Vec<Vec<SqlVal>>, Self::Error> {
            Ok(Vec::new())
        }

        fn execute_raw(&mut self, query: &BuiltQuery) -> Result<u64, Self::Error> {
            self.statements.push(query.sql.clone());
            Ok(0)
        }
    }

    impl TransactionBackend for MockTransactionBackend {}

    #[test]
    fn transaction_statements_render_expected_sql() {
        let mut backend = MockTransactionBackend::default();
        backend.begin(None).expect("begin");
        backend
            .begin(Some(IsolationLevel::Serializable))
            .expect("begin with isolation");
        backend.savepoint("rurqe_sp_1").expect("savepoint");
        backend.release_savepoint("rurqe_sp_1").expect("release");
        backend
            .rollback_to_savepoint("rurqe_sp_2")
            .expect("rollback to savepoint");
        backend.commit().expect("commit");
        backend.rollback().expect("rollback");

        assert_eq!(
            backend.statements,
            vec![
                "BEGIN",
                "BEGIN ISOLATION LEVEL SERIALIZABLE",
                "SAVEPOINT rurqe_sp_1",
                "RELEASE SAVEPOINT rurqe_sp_1",
                "ROLLBACK TO SAVEPOINT rurqe_sp_2",
                "COMMIT",
                "ROLLBACK",
            ]
        );
    }

    #[test]
    fn isolation_levels_render_canonical_clauses() {
        assert_eq!(IsolationLevel::ReadCommitted.as_str(), "READ COMMITTED");
        assert_eq!(IsolationLevel::RepeatableRead.as_str(), "REPEATABLE READ");
        assert_eq!(IsolationLevel::Serializable.as_str(), "SERIALIZABLE");
    }

    #[test]
    fn transaction_context_hands_out_nested_savepoints() {
        let mut context = TransactionContext::new();
        assert_eq!(context.depth(), 0);
        assert!(!context.is_nested());
        assert_eq!(context.current_savepoint(), None);

        let outer = context.open_savepoint();
        let inner = context.open_savepoint();
        assert_ne!(outer, inner);
        assert_eq!(context.depth(), 2);
        assert!(context.is_nested());
        assert_eq!(context.current_savepoint(), Some(inner.as_str()));

        assert_eq!(context.close_savepoint(), Some(inner));
        assert_eq!(context.current_savepoint(), Some(outer.as_str()));
        assert_eq!(context.close_savepoint(), Some(outer));
        assert_eq!(context.close_savepoint(), None);
    }

    #[test]
    fn window_functions_render_expected_over_clauses() {
        let window = Window::partition_by(UserId).order_by_asc(UserName);

        let built = SelectQuery::<MockDialect>::new()
            .from::<User>()
            .select((
                UserId,
                row_number().over(window.clone()),
                rank().over(window.clone()),
                dense_rank().over(window.clone()),
                lag(UserName).offset(2).over(Window::order_by_desc(UserId)),
                lead(UserName).over(Window::empty()),
                Expr::count_all().over(Window::partition_by(UserId)),
            ))
            .build();

        assert_eq!(
            built.sql,
            "SELECT \"id\", ROW_NUMBER() OVER (PARTITION BY \"id\" ORDER BY \"name\" ASC), \
             RANK() OVER (PARTITION BY \"id\" ORDER BY \"name\" ASC), \
             DENSE_RANK() OVER (PARTITION BY \"id\" ORDER BY \"name\" ASC), \
             LAG(\"name\", 2) OVER (ORDER BY \"id\" DESC), \
             LEAD(\"name\") OVER (), \
             COUNT(*) OVER (PARTITION BY \"id\") \
             FROM \"users\""
        );
    }

    #[test]
    fn single_cte_renders_with_clause_and_binds_sequential_parameters() {
        let active = SelectQuery::<MockDialect>::new()
            .from::<User>()
            .select((UserId, UserName))
            .where_(Expr::column::<UserId>().gt(10i64));
        let active_cte = cte::<User, _, _, _>(active);

        let built = SelectQuery::<MockDialect>::new()
            .with(active_cte)
            .from::<User>()
            .select((UserId, UserName))
            .where_(Expr::column::<UserName>().eq("root"))
            .build();

        assert_eq!(
            built.sql,
            "WITH \"users\" AS (SELECT \"id\", \"name\" FROM \"users\" WHERE (\"id\" > $1)) \
             SELECT \"id\", \"name\" FROM \"users\" WHERE (\"name\" = $2)"
        );
        assert_eq!(
            built.params,
            vec![SqlVal::I64(10), SqlVal::Text("root".to_owned())]
        );
    }

    #[test]
    fn multiple_ctes_render_comma_separated_clauses() {
        let u_sub = SelectQuery::<MockDialect>::new()
            .from::<User>()
            .select((UserId,))
            .where_(Expr::column::<UserId>().gt(1i64));
        let o_sub = SelectQuery::<MockDialect>::new()
            .from::<Order>()
            .select((OrderId,))
            .where_(Expr::column::<OrderTotal>().gt(100.0));

        let built = SelectQuery::<MockDialect>::new()
            .with(cte_named("active_users", u_sub))
            .with(cte_named("large_orders", o_sub))
            .from::<User>()
            .select((UserId,))
            .where_(Expr::column::<UserId>().eq(7i64))
            .build();

        assert_eq!(
            built.sql,
            "WITH \"active_users\" AS (SELECT \"id\" FROM \"users\" WHERE (\"id\" > $1)), \
             \"large_orders\" AS (SELECT \"id\" FROM \"orders\" WHERE (\"total\" > $2)) \
             SELECT \"id\" FROM \"users\" WHERE (\"id\" = $3)"
        );
        assert_eq!(
            built.params,
            vec![SqlVal::I64(1), SqlVal::F64(100.0), SqlVal::I64(7)]
        );
    }

    #[test]
    fn cte_binds_to_table_marker_and_participates_in_joins() {
        crate::table! {
            ActiveSummary => "active_summary" {
                SummaryUserId => "user_id": i64,
                SummaryTotal => "total": f64,
            }
        }

        let summary_query = SelectQuery::<MockDialect>::new()
            .from::<Order>()
            .select((OrderUserId, OrderTotal))
            .where_(Expr::column::<OrderTotal>().gt(50.0));

        let summary_cte = summary_query.as_cte::<ActiveSummary>();

        let built = SelectQuery::<MockDialect>::new()
            .with(summary_cte)
            .from::<User>()
            .inner_join::<ActiveSummary>(
                Expr::column::<UserId>().eq(Expr::column::<SummaryUserId>()),
            )
            .select((UserId, UserName, SummaryTotal))
            .where_(Expr::column::<SummaryTotal>().lt(500.0))
            .build();

        assert_eq!(
            built.sql,
            "WITH \"active_summary\" AS (SELECT \"user_id\", \"total\" FROM \"orders\" WHERE (\"total\" > $1)) \
             SELECT \"id\", \"name\", \"total\" FROM \"users\" \
             INNER JOIN \"active_summary\" ON (\"id\" = \"user_id\") \
             WHERE (\"total\" < $2)"
        );
        assert_eq!(built.params, vec![SqlVal::F64(50.0), SqlVal::F64(500.0)]);
    }

    #[test]
    fn raw_sql_renders_sequential_placeholders_in_query_clauses() {
        let raw_pred: Expr<bool> =
            Expr::raw("strftime('%Y-%m', date) = ?", vec![SqlVal::Text("2024-01".into())]);

        let built = SelectQuery::<MockDialect>::new()
            .from::<User>()
            .select((UserId, Expr::<i64>::raw("COUNT(*)", vec![])))
            .where_(Expr::column::<UserId>().gt(10i64))
            .where_(raw_pred)
            .build();

        assert_eq!(
            built.sql,
            "SELECT \"id\", COUNT(*) FROM \"users\" WHERE ((\"id\" > $1) AND strftime('%Y-%m', date) = $2)"
        );
        assert_eq!(
            built.params,
            vec![SqlVal::I64(10), SqlVal::Text("2024-01".into())]
        );
    }

    #[test]
    fn subquery_predicates_render_nested_queries_with_sequential_parameters() {
        let large_orders = SelectQuery::<MockDialect>::new()
            .from::<Order>()
            .select((OrderUserId,))
            .where_(Expr::column::<OrderTotal>().gt(100.0));

        let recent_orders = SelectQuery::<MockDialect>::new()
            .from::<Order>()
            .select((OrderId,))
            .where_(Expr::column::<OrderTotal>().lt(10.0));

        let built = SelectQuery::<MockDialect>::new()
            .from::<User>()
            .select((UserId,))
            .where_(Expr::column::<UserId>().in_subquery(large_orders))
            .where_(Expr::not_exists(recent_orders))
            .build();

        assert_eq!(
            built.sql,
            "SELECT \"id\" FROM \"users\" WHERE ((\"id\" IN (SELECT \"user_id\" FROM \"orders\" \
             WHERE (\"total\" > $1))) AND (NOT EXISTS (SELECT \"id\" FROM \"orders\" WHERE \
             (\"total\" < $2))))"
        );
        assert_eq!(built.params, vec![SqlVal::F64(100.0), SqlVal::F64(10.0)]);
    }

    #[test]
    fn not_in_subquery_renders_negated_membership() {
        let totals = SelectQuery::<MockDialect>::new()
            .from::<Order>()
            .select((OrderTotal,));

        let built = SelectQuery::<MockDialect>::new()
            .from::<Order>()
            .select((OrderId,))
            .where_(Expr::column::<OrderTotal>().not_in_subquery(totals))
            .build();

        assert_eq!(
            built.sql,
            "SELECT \"id\" FROM \"orders\" WHERE (\"total\" NOT IN (SELECT \"total\" FROM \"orders\"))"
        );
        assert!(built.params.is_empty());
    }

    #[test]
    fn union_renders_combined_query_and_binds_parameters_in_order() {
        let other = SelectQuery::<MockDialect>::new()
            .from::<User>()
            .select((UserId, UserName))
            .where_(Expr::column::<UserId>().gt(10i64));

        let built = SelectQuery::<MockDialect>::new()
            .from::<User>()
            .select((UserId, UserName))
            .where_(Expr::column::<UserId>().lt(5i64))
            .union(other)
            .build();

        assert_eq!(
            built.sql,
            "SELECT \"id\", \"name\" FROM \"users\" WHERE (\"id\" < $1) \
             UNION SELECT \"id\", \"name\" FROM \"users\" WHERE (\"id\" > $2)"
        );
        assert_eq!(built.params, vec![SqlVal::I64(5), SqlVal::I64(10)]);
    }

    #[test]
    fn set_operation_keywords_render_expected_sql() {
        fn base() -> SelectQuery<MockDialect, User, (i64,)> {
            SelectQuery::<MockDialect>::new()
                .from::<User>()
                .select((UserId,))
        }

        assert_eq!(
            base().union(base()).build().sql,
            "SELECT \"id\" FROM \"users\" UNION SELECT \"id\" FROM \"users\""
        );
        assert_eq!(
            base().union_all(base()).build().sql,
            "SELECT \"id\" FROM \"users\" UNION ALL SELECT \"id\" FROM \"users\""
        );
        assert_eq!(
            base().intersect(base()).build().sql,
            "SELECT \"id\" FROM \"users\" INTERSECT SELECT \"id\" FROM \"users\""
        );
        assert_eq!(
            base().except(base()).build().sql,
            "SELECT \"id\" FROM \"users\" EXCEPT SELECT \"id\" FROM \"users\""
        );
    }

    #[test]
    fn chained_set_operations_keep_trailing_order_by_and_limit_on_the_union() {
        fn base() -> SelectQuery<MockDialect, User, (i64,)> {
            SelectQuery::<MockDialect>::new()
                .from::<User>()
                .select((UserId,))
        }

        let built = base()
            .union(base())
            .union_all(base())
            .order_by_asc(UserId)
            .limit(3)
            .build();

        assert_eq!(
            built.sql,
            "SELECT \"id\" FROM \"users\" UNION SELECT \"id\" FROM \"users\" \
             UNION ALL SELECT \"id\" FROM \"users\" ORDER BY \"id\" ASC LIMIT 3"
        );
        assert!(built.params.is_empty());
    }

    #[test]
    fn union_query_is_executable_and_decodes_rows() {
        let query = SelectQuery::<MockDialect>::new()
            .from::<User>()
            .select((UserId,))
            .union(
                SelectQuery::<MockDialect>::new()
                    .from::<User>()
                    .select((UserId,)),
            );

        let mut backend = MockBackend {
            rows: vec![vec![SqlVal::I64(1)]],
            ..MockBackend::default()
        };

        let rows = backend.fetch_all(&query).expect("rows decode");
        assert_eq!(rows, vec![(1i64,)]);
        assert_eq!(backend.executed, vec![query.build()]);
    }

    crate::table_alias! {
        /// The `u` alias.
        UserAlias => "u"
    }

    crate::table_alias! {
        /// The `o` alias.
        OrderAlias => "o"
    }

    #[test]
    fn aliased_table_renders_as_clause_and_qualifies_columns() {
        let built = SelectQuery::<MockDialect>::new()
            .from::<Aliased<User, UserAlias>>()
            .select((AliasedCol::<UserAlias, UserId>::new(), UserName))
            .where_(Expr::column::<AliasedCol<UserAlias, UserId>>().gt(1i64))
            .build();

        assert_eq!(
            built.sql,
            "SELECT \"u\".\"id\", \"name\" FROM \"users\" AS \"u\" WHERE (\"u\".\"id\" > $1)"
        );
        assert_eq!(built.params, vec![SqlVal::I64(1)]);
    }

    #[test]
    fn aliased_table_participates_in_joins() {
        let built = SelectQuery::<MockDialect>::new()
            .from::<User>()
            .inner_join::<Aliased<Order, OrderAlias>>(
                Expr::column::<OrderUserId>()
                    .eq(Expr::column::<AliasedCol<OrderAlias, OrderUserId>>()),
            )
            .select((UserId, AliasedCol::<OrderAlias, OrderTotal>::new()))
            .build();

        assert_eq!(
            built.sql,
            "SELECT \"id\", \"o\".\"total\" FROM \"users\" \
             INNER JOIN \"orders\" AS \"o\" ON (\"user_id\" = \"o\".\"user_id\")"
        );
        assert!(built.params.is_empty());
    }

    #[test]
    fn left_joined_aliased_table_lifts_columns_into_options() {
        let query = SelectQuery::<MockDialect>::new()
            .from::<User>()
            .left_join::<Aliased<Order, OrderAlias>>(
                Expr::column::<OrderUserId>().eq(Expr::column::<UserId>()),
            );
        let total = query.nullable::<AliasedCol<OrderAlias, OrderTotal>>();

        let built = query.select((UserId, total)).build();

        assert_eq!(
            built.sql,
            "SELECT \"id\", \"o\".\"total\" FROM \"users\" \
             LEFT JOIN \"orders\" AS \"o\" ON (\"user_id\" = \"id\")"
        );
    }

    crate::table! {
        RecentOrder => "recent" {
            RecentTotal => "total": f64,
        }
    }

    fn latest_order_per_user_scope() -> SelectQuery<MockDialect, Order, (f64,)> {
        SelectQuery::<MockDialect>::new()
            .from::<Order>()
            .select((OrderTotal,))
            .where_(Expr::column::<OrderUserId>().eq(Expr::column::<UserId>()))
            .limit(1)
    }

    #[test]
    fn cross_join_lateral_renders_inline_correlated_subquery() {
        let built = SelectQuery::<MockDialect>::new()
            .from::<User>()
            .cross_join_lateral(latest_order_per_user_scope().as_lateral::<RecentOrder>())
            .select((UserId, RecentTotal))
            .build();

        assert_eq!(
            built.sql,
            "SELECT \"id\", \"total\" FROM \"users\" CROSS JOIN LATERAL \
             (SELECT \"total\" FROM \"orders\" WHERE (\"user_id\" = \"id\") LIMIT 1) AS \"recent\""
        );
        assert!(built.params.is_empty());
    }

    #[test]
    fn left_join_lateral_renders_on_true_and_lifts_columns_into_options() {
        let query = SelectQuery::<MockDialect>::new()
            .from::<User>()
            .left_join_lateral(latest_order_per_user_scope().as_lateral::<RecentOrder>());
        let total = query.nullable::<RecentTotal>();

        let built = query.select((UserId, total)).build();

        assert_eq!(
            built.sql,
            "SELECT \"id\", \"total\" FROM \"users\" LEFT JOIN LATERAL \
             (SELECT \"total\" FROM \"orders\" WHERE (\"user_id\" = \"id\") LIMIT 1) AS \"recent\" ON true"
        );
    }

    #[test]
    fn lateral_join_binds_subquery_parameters_before_the_outer_filter() {
        let sub = SelectQuery::<MockDialect>::new()
            .from::<Order>()
            .select((OrderTotal,))
            .where_(Expr::column::<OrderUserId>().eq(Expr::column::<UserId>()))
            .where_(Expr::column::<OrderTotal>().gt(10.0))
            .limit(1);

        let built = SelectQuery::<MockDialect>::new()
            .from::<User>()
            .cross_join_lateral(sub.as_lateral::<RecentOrder>())
            .select((UserId, RecentTotal))
            .where_(Expr::column::<UserId>().gt(1i64))
            .build();

        assert_eq!(
            built.sql,
            "SELECT \"id\", \"total\" FROM \"users\" CROSS JOIN LATERAL \
             (SELECT \"total\" FROM \"orders\" WHERE ((\"user_id\" = \"id\") AND (\"total\" > $1)) \
             LIMIT 1) AS \"recent\" WHERE (\"id\" > $2)"
        );
        assert_eq!(built.params, vec![SqlVal::F64(10.0), SqlVal::I64(1)]);
    }
}
