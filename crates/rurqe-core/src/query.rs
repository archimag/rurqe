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

use core::fmt::Write;
use core::marker::PhantomData;

use crate::dialect::Dialect;
use crate::expr::{BinaryOp, Expr, ExprKind, SqlVal};
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
    joins: Vec<ExprKind>,
    filter: Option<ExprKind>,
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
        self.joins.push(on.into_kind());
        self.retype()
    }

    fn retype<D2: Dialect, Tables2, Output2>(self) -> SelectQuery<D2, Tables2, Output2> {
        SelectQuery {
            projections: self.projections,
            joins: self.joins,
            filter: self.filter,
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
        for (index, on) in self.joins.iter().enumerate() {
            sql.push_str(" INNER JOIN ");
            D::push_quoted_identifier(&mut sql, tables[index + 1]);
            sql.push_str(" ON ");
            render_expr::<D>(&mut sql, &mut params, on);
        }

        if let Some(filter) = &self.filter {
            sql.push_str(" WHERE ");
            render_expr::<D>(&mut sql, &mut params, filter);
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_scope::<Joined<Joined<User, Order>, User>>();
    }
}
