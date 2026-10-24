//! Rendering adapters of the `SELECT` query.
//!
//! The builder records clause data only, so rendering it is a single pass over
//! the statement the query describes: the adapters here convert the builder into
//! the dialect-neutral [`SelectAst`] and hand the AST to the renderer.

use crate::dialect::Dialect;
use crate::expr::{Expr, SelectAst, SqlVal, Subquery};
use crate::query::cte::{Cte, DerivedTable, Lateral};
use crate::query::scope::TableScope;
use crate::schema::{Table, TableRef};

use super::{BuiltQuery, HasProjection, ProjectionPresent, SelectQuery};

impl<D: Dialect, Tables: TableScope, Output, P: ProjectionPresent, Lock>
    SelectQuery<D, Tables, Output, P, Lock>
{
    /// Describes the query as a dialect-neutral statement AST.
    pub fn to_ast(&self) -> SelectAst {
        let from = self.from.or_else(|| {
            let mut tables: Vec<TableRef> = Vec::new();
            Tables::push_table_refs(&mut tables);
            tables.first().copied()
        });

        SelectAst {
            ctes: self.ctes.clone(),
            distinct: self.distinct,
            distinct_on: self.distinct_on.clone(),
            projections: self.projections.clone(),
            from,
            joins: self.joins.clone(),
            derived: self.derived.clone(),
            filter: self.filter.clone(),
            group_by: self.group_by.clone(),
            having: self.having.clone(),
            order_by: self.order_by.clone(),
            limit: self.limit,
            offset: self.offset,
            set_operations: self.set_operations.clone(),
            compound_order_by: self.compound_order_by.clone(),
            compound_limit: self.compound_limit,
            compound_offset: self.compound_offset,
            lock: self.lock,
        }
    }

    /// Renders the query statement and parameters into the target buffers.
    pub fn render_into(&self, sql: &mut String, params: &mut Vec<SqlVal>) {
        self.to_ast().render::<D>(sql, params);
    }

    /// Renders the query into SQL text and its ordered parameter list.
    ///
    /// The method is reachable only once the query records a projection, so a
    /// query rendered without `select` is rejected at compile time in debug and
    /// release builds alike; see [`ProjectionPresent`].
    pub fn build(&self) -> BuiltQuery {
        let mut sql = String::new();
        let mut params = Vec::new();
        self.render_into(&mut sql, &mut params);
        BuiltQuery { sql, params }
    }

    /// Wraps the query into a [`Subquery`] carrying this statement as plain data.
    pub fn into_subquery(self) -> Subquery {
        Subquery::new(self.to_ast())
    }

    /// Builds an `EXISTS (subquery)` predicate over this query.
    pub fn exists(&self) -> Expr<bool> {
        Expr::exists(Subquery::new(self.to_ast()))
    }

    /// Builds a `NOT EXISTS (subquery)` predicate over this query.
    pub fn not_exists(&self) -> Expr<bool> {
        Expr::not_exists(Subquery::new(self.to_ast()))
    }
}

impl<D: Dialect, Tables: TableScope, Output> SelectQuery<D, Tables, Output, HasProjection> {
    /// Converts this query into a CTE for the table `T`.
    pub fn as_cte<T: Table>(self) -> Cte<T, Output, D> {
        Cte::new(self)
    }

    /// Exposes this query as a derived table referenced through the marker `T`.
    ///
    /// The marker declares the columns of the projected rows, which the caller
    /// states by choosing it: the relation is then referenced like a stored table
    /// — `"name"."column"` — while the database materialises nothing.
    pub fn as_table<T: Table>(self) -> DerivedTable<T, Output, D> {
        DerivedTable::new(self)
    }

    /// Converts this query into a named CTE with alias `name`.
    pub fn as_named_cte(self, name: &'static str) -> Cte<(), Output, D> {
        Cte::named(name, self)
    }

    /// Converts this query into a `LATERAL` subquery exposed under the table marker `T`.
    pub fn as_lateral<T: Table>(self) -> Lateral<T, Output, D> {
        Lateral::new(self)
    }
}

impl<D: Dialect, Tables: TableScope, Output> From<SelectQuery<D, Tables, Output, HasProjection>>
    for Subquery
{
    fn from(query: SelectQuery<D, Tables, Output, HasProjection>) -> Self {
        query.into_subquery()
    }
}
