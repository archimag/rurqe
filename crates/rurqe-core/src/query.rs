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

use crate::dialect::Dialect;
use crate::dml::{DeleteQuery, Filtered, InsertQuery, UpdateQuery};
use crate::expr::{BinaryOp, Expr, ExprKind, SqlVal};
use crate::row::FromRow;
use crate::schema::{Column, Table};

/// A list of selected columns whose Rust value types form the query row type.
///
/// The trait is implemented for column tuples, so `(UserId,)` yields a `(i64,)`
/// row and `(UserId, UserName)` yields a `(i64, String)` row.
#[diagnostic::on_unimplemented(
    message = "`{Self}` is not a supported selection",
    label = "select a column or a tuple of columns",
    note = "selections are column markers written as tuples, such as `(UserId,)` or `(UserId, UserName)`"
)]
pub trait Selection {
    /// The tuple of Rust values produced for a single row.
    type Row;

    /// Appends the SQL names of the selected columns to `out` in selection order.
    fn push_column_names(&self, out: &mut Vec<&'static str>);
}

macro_rules! impl_selection {
    ($(($column:ident, $rust_type:ident)),+) => {
        impl<$($column: Column),+> Selection for ($($column,)+) {
            type Row = ($($column::$rust_type,)+);

            fn push_column_names(&self, out: &mut Vec<&'static str>) {
                $(out.push($column::NAME);)+
            }
        }
    };
}

impl_selection!((C1, RustType));
impl_selection!((C1, RustType), (C2, RustType));
impl_selection!((C1, RustType), (C2, RustType), (C3, RustType));
impl_selection!((C1, RustType), (C2, RustType), (C3, RustType), (C4, RustType));
impl_selection!(
    (C1, RustType),
    (C2, RustType),
    (C3, RustType),
    (C4, RustType),
    (C5, RustType)
);
impl_selection!(
    (C1, RustType),
    (C2, RustType),
    (C3, RustType),
    (C4, RustType),
    (C5, RustType),
    (C6, RustType)
);

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
}

impl<T: Table> TableScope for T {
    fn push_table_names(out: &mut Vec<&'static str>) {
        out.push(T::NAME);
    }
}

/// The flavour of join used to bring a table into the query scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum JoinKind {
    /// `INNER JOIN`, which only keeps rows matched in the joined table.
    Inner,
    /// `LEFT JOIN`, which keeps unmatched rows and nulls the joined columns.
    Left,
}

impl JoinKind {
    /// The SQL text of the join keywords.
    pub const fn as_str(self) -> &'static str {
        match self {
            JoinKind::Inner => "INNER JOIN",
            JoinKind::Left => "LEFT JOIN",
        }
    }
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
}

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

/// A `SELECT` query over the table `Tables` producing rows of type `Output`.
///
/// `Tables` remains `()` until [`SelectQuery::from`] introduces the queried
/// table and `Output` remains `()` until [`SelectQuery::select`] fixes the row
/// type, which keeps the builder signature flat while tracking both the scope
/// and the result type at compile time.
pub struct SelectQuery<D: Dialect, Tables = (), Output = ()> {
    projections: Vec<&'static str>,
    joins: Vec<(JoinKind, ExprKind)>,
    filter: Option<ExprKind>,
    group_by: Vec<&'static str>,
    having: Option<ExprKind>,
    order_by: Vec<(&'static str, OrderDirection)>,
    limit: Option<u32>,
    offset: Option<u32>,
    _marker: QueryMarker<D, Tables, Output>,
}

/// Phantom marker carrying the dialect, scope, and row type of a query.
///
/// The parameters are wrapped in a function result, so they are treated as
/// covariant and no drop-check obligations are introduced.
type QueryMarker<D, Tables, Output> = PhantomData<fn() -> (D, Tables, Output)>;

impl<D: Dialect> SelectQuery<D, (), ()> {
    /// Creates a query without a table, projections, filter, or pagination.
    pub fn new() -> Self {
        SelectQuery {
            projections: Vec::new(),
            joins: Vec::new(),
            filter: None,
            group_by: Vec::new(),
            having: None,
            order_by: Vec::new(),
            limit: None,
            offset: None,
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
    pub fn select<S: Selection>(mut self, selection: S) -> SelectQuery<D, Tables, S::Row> {
        selection.push_column_names(&mut self.projections);
        self.retype()
    }
}

impl<D: Dialect, Tables, Output> SelectQuery<D, Tables, Output> {
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
        self.joins.push((JoinKind::Inner, on.into_kind()));
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
        self.joins.push((JoinKind::Left, on.into_kind()));
        self.retype()
    }

    fn retype<D2: Dialect, Tables2, Output2>(self) -> SelectQuery<D2, Tables2, Output2> {
        SelectQuery {
            projections: self.projections,
            joins: self.joins,
            filter: self.filter,
            group_by: self.group_by,
            having: self.having,
            order_by: self.order_by,
            limit: self.limit,
            offset: self.offset,
            _marker: PhantomData,
        }
    }
}

impl<D: Dialect, Tables: TableScope, Output> SelectQuery<D, Tables, Output> {
    /// Renders the query into SQL text and its ordered parameter list.
    pub fn build(&self) -> BuiltQuery {
        assert!(
            !self.projections.is_empty(),
            "a SELECT query requires at least one projection; call `select` before `build`"
        );

        let mut tables = Vec::new();
        Tables::push_table_names(&mut tables);
        assert_eq!(
            tables.len(),
            self.joins.len() + 1,
            "every joined table requires an ON condition"
        );

        let mut sql = String::from("SELECT ");
        for (index, name) in self.projections.iter().enumerate() {
            if index > 0 {
                sql.push_str(", ");
            }
            D::push_quoted_identifier(&mut sql, name);
        }

        sql.push_str(" FROM ");
        D::push_quoted_identifier(&mut sql, tables[0]);

        let mut params = Vec::new();
        for (index, (kind, on)) in self.joins.iter().enumerate() {
            sql.push(' ');
            sql.push_str(kind.as_str());
            sql.push(' ');
            D::push_quoted_identifier(&mut sql, tables[index + 1]);
            sql.push_str(" ON ");
            render_expr::<D>(&mut sql, &mut params, on);
        }

        if let Some(filter) = &self.filter {
            sql.push_str(" WHERE ");
            render_expr::<D>(&mut sql, &mut params, filter);
        }

        if !self.group_by.is_empty() {
            sql.push_str(" GROUP BY ");
            for (index, name) in self.group_by.iter().enumerate() {
                if index > 0 {
                    sql.push_str(", ");
                }
                D::push_quoted_identifier(&mut sql, name);
            }
        }

        if let Some(having) = &self.having {
            sql.push_str(" HAVING ");
            render_expr::<D>(&mut sql, &mut params, having);
        }

        if !self.order_by.is_empty() {
            sql.push_str(" ORDER BY ");
            for (index, (name, direction)) in self.order_by.iter().enumerate() {
                if index > 0 {
                    sql.push_str(", ");
                }
                D::push_quoted_identifier(&mut sql, name);
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

        BuiltQuery { sql, params }
    }
}

/// Renders `expr` into `sql`, appending every literal as a bound parameter.
fn render_expr<D: Dialect>(sql: &mut String, params: &mut Vec<SqlVal>, expr: &ExprKind) {
    match expr {
        ExprKind::Column { name } => D::push_quoted_identifier(sql, name),
        ExprKind::Value(value) => {
            params.push(value.clone());
            D::push_placeholder(sql, params.len());
        }
        ExprKind::Binary { op, lhs, rhs } => {
            sql.push('(');
            render_expr::<D>(sql, params, lhs);
            sql.push(' ');
            sql.push_str(op.as_str());
            sql.push(' ');
            render_expr::<D>(sql, params, rhs);
            sql.push(')');
        }
        ExprKind::Aggregate { func, arg } => {
            sql.push_str(func.as_str());
            sql.push('(');
            match arg {
                Some(inner) => render_expr::<D>(sql, params, inner),
                None => sql.push('*'),
            }
            sql.push(')');
        }
    }
}

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dml::ColumnAssign;

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
}
