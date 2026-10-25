//! Rendering adapters of the `SELECT` query.
//!
//! The builder records clause data only, so rendering it is a single pass over
//! the statement the query describes: the adapters here convert the builder into
//! the dialect-neutral [`SelectAst`] and hand the AST to the renderer.

use crate::dialect::Dialect;
use crate::expr::{
    Expr, ExprKind, NonAggregate, SelectAst, SqlVal, StatementRef, Subquery, SubqueryOp,
};
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

    /// Borrows the recorded clauses of the query as a statement view.
    ///
    /// The view references the clause vectors the builder already owns, so
    /// validating and rendering a query reads the recorded statement in place
    /// instead of cloning its clauses — and the subqueries embedded in them — on
    /// every build. [`SelectQuery::to_ast`] stays the owning conversion the
    /// embedding entry points need.
    pub(crate) fn statement(&self) -> StatementRef<'_> {
        let from = self.from.or_else(|| {
            let mut tables: Vec<TableRef> = Vec::new();
            Tables::push_table_refs(&mut tables);
            tables.first().copied()
        });

        StatementRef {
            ctes: &self.ctes,
            distinct: self.distinct,
            distinct_on: &self.distinct_on,
            projections: &self.projections,
            projection_aliases: None,
            from,
            joins: &self.joins,
            derived: &self.derived,
            filter: self.filter.as_ref(),
            group_by: &self.group_by,
            having: self.having.as_ref(),
            order_by: &self.order_by,
            limit: self.limit,
            offset: self.offset,
            set_operations: &self.set_operations,
            compound_order_by: &self.compound_order_by,
            compound_limit: self.compound_limit,
            compound_offset: self.compound_offset,
            lock: self.lock,
        }
    }

    /// Validates pre-flight SQL semantics and renders the query into the target buffers.
    ///
    /// The statement is validated before a single byte reaches `sql` or `params`,
    /// exactly like [`SelectQuery::build`], so a query that violates a semantic
    /// rule is reported as a [`BuildError`](crate::query::BuildError) instead of
    /// being rendered and dispatched to the engine.
    pub fn render_into(
        &self,
        sql: &mut String,
        params: &mut Vec<SqlVal>,
    ) -> Result<(), crate::query::BuildError> {
        let statement = self.statement();
        statement.validate()?;
        statement.render::<D>(sql, params);
        Ok(())
    }

    /// Validates pre-flight SQL semantics and renders the query into SQL text and its ordered parameter list.
    ///
    /// The method is reachable only once the query records a projection, so a
    /// query rendered without `select` is rejected at compile time in debug and
    /// release builds alike; see [`ProjectionPresent`].
    pub fn build(&self) -> Result<BuiltQuery, crate::query::BuildError> {
        let statement = self.statement();
        statement.validate()?;
        let mut sql = String::new();
        let mut params = Vec::new();
        statement.render::<D>(&mut sql, &mut params);
        Ok(BuiltQuery { sql, params })
    }

    /// Wraps the query into a [`Subquery`] carrying this statement as plain data.
    pub fn into_subquery(self) -> Subquery {
        Subquery::new(self.to_ast())
    }

    /// Builds an `EXISTS (subquery)` predicate over this query.
    pub fn exists(&self) -> Expr<bool, (), NonAggregate, D> {
        Expr::new(ExprKind::Subquery {
            op: SubqueryOp::Exists,
            operand: None,
            query: Subquery::new(self.to_ast()),
            negated: false,
        })
    }

    /// Builds a `NOT EXISTS (subquery)` predicate over this query.
    pub fn not_exists(&self) -> Expr<bool, (), NonAggregate, D> {
        Expr::new(ExprKind::Subquery {
            op: SubqueryOp::Exists,
            operand: None,
            query: Subquery::new(self.to_ast()),
            negated: true,
        })
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
