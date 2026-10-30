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

use super::{BuiltQuery, HasProjection, ProjectionPresent, SelectQuery, Unlocked};

impl<D: Dialect, Tables: TableScope, Output, P: ProjectionPresent, Lock>
    SelectQuery<D, Tables, Output, P, Lock>
{
    /// Describes the query as a dialect-neutral statement AST.
    ///
    /// The conversion copies every clause the builder records, which is what a
    /// caller keeping the query needs. A builder that is consumed instead — a
    /// subquery, a CTE, a derived table, or a lateral join — takes
    /// [`SelectQuery::into_ast`], which hands the clauses over without copying
    /// them.
    pub fn to_ast(&self) -> SelectAst {
        self.clone().into_ast()
    }

    /// Consumes the query into its dialect-neutral statement AST.
    ///
    /// Every clause the builder owns is moved into the statement, so the
    /// conversion copies neither the clause vectors nor the nested statements and
    /// expression trees they hold: an `EXISTS` predicate, a CTE, a lateral join,
    /// or a derived table built from a query that is dropped right afterwards
    /// allocates the statement once instead of twice.
    pub fn into_ast(self) -> SelectAst {
        let from = self.from.or_else(|| {
            let mut tables: Vec<TableRef> = Vec::new();
            Tables::push_table_refs(&mut tables);
            tables.first().copied()
        });

        SelectAst {
            ctes: self.ctes,
            distinct: self.distinct,
            distinct_on: self.distinct_on,
            projections: self.projections,
            from,
            joins: self.joins,
            derived: self.derived,
            filter: self.filter,
            group_by: self.group_by,
            having: self.having,
            order_by: self.order_by,
            limit: self.limit,
            offset: self.offset,
            set_operations: self.set_operations,
            compound_order_by: self.compound_order_by,
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
        statement.render::<D>(sql, params)?;
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
        statement.render::<D>(&mut sql, &mut params)?;
        Ok(BuiltQuery { sql, params })
    }
}

/// Conversions nesting the query inside another statement.
///
/// A nested statement renders inside a clause of the statement embedding it, so the
/// rows it contributes are the rows of *that* statement: a row-level lock it could
/// claim would belong to no statement of its own. The conversions therefore accept
/// an [`Unlocked`] query alone, which leaves the typestate builders
/// [`SelectQuery::for_update`] and [`SelectQuery::for_share`] as the only way to
/// request a lock, while [`SelectQuery::build`] still renders a locked statement and
/// [`SelectQuery::to_ast`] still describes one.
impl<D: Dialect, Tables: TableScope, Output, P: ProjectionPresent, Lock: Unlocked>
    SelectQuery<D, Tables, Output, P, Lock>
{
    /// Wraps the query into a [`Subquery`] carrying this statement as plain data.
    ///
    /// The query is consumed, so the statement moves its clauses into the
    /// subquery instead of copying them. It also has to be unlocked: a statement
    /// that claims its rows for its own transaction is executed on its own rather
    /// than nested into another statement — see [`Unlocked`].
    pub fn into_subquery(self) -> Subquery {
        Subquery::new(self.into_ast())
    }

    /// Builds an `EXISTS (subquery)` predicate over this query.
    ///
    /// The predicate is built from a copy of the query, which is what a caller
    /// that keeps using it needs. A query that is dropped right afterwards takes
    /// [`SelectQuery::into_exists`] instead, which moves the statement rather
    /// than copying it. Like every conversion into a nested position, the
    /// predicate accepts an unlocked query alone — see [`Unlocked`].
    pub fn exists(&self) -> Expr<bool, (), NonAggregate, D> {
        self.clone().into_exists()
    }

    /// Builds a `NOT EXISTS (subquery)` predicate over this query.
    ///
    /// The predicate is built from a copy of the query, exactly like
    /// [`SelectQuery::exists`]; [`SelectQuery::into_not_exists`] is the
    /// consuming form.
    pub fn not_exists(&self) -> Expr<bool, (), NonAggregate, D> {
        self.clone().into_not_exists()
    }

    /// Builds an `EXISTS (subquery)` predicate over this query, consuming it.
    ///
    /// The nested statement takes the clauses the builder owns, so a predicate
    /// assembled from a query that is dropped right afterwards copies neither the
    /// clause vectors nor the statements and expressions they hold.
    pub fn into_exists(self) -> Expr<bool, (), NonAggregate, D> {
        Expr::new(ExprKind::Subquery {
            op: SubqueryOp::Exists,
            operand: None,
            query: Subquery::new(self.into_ast()),
            negated: false,
        })
    }

    /// Builds a `NOT EXISTS (subquery)` predicate over this query, consuming it.
    ///
    /// The nested statement takes the clauses the builder owns, exactly like
    /// [`SelectQuery::into_exists`].
    pub fn into_not_exists(self) -> Expr<bool, (), NonAggregate, D> {
        Expr::new(ExprKind::Subquery {
            op: SubqueryOp::Exists,
            operand: None,
            query: Subquery::new(self.into_ast()),
            negated: true,
        })
    }
}

impl<D: Dialect, Tables: TableScope, Output> SelectQuery<D, Tables, Output, HasProjection> {
    /// Converts this query into a CTE for the table `T`.
    ///
    /// The conversion is reachable only once the query projects its rows, so a
    /// query without a projection has no statement to publish as a named
    /// relation:
    ///
    /// ```compile_fail
    /// use rurqe_core::dialect::Dialect;
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
    ///     }
    /// }
    ///
    /// fn main() {
    ///     // Fails to compile: the query records no projection, so there is no
    ///     // statement to publish as a named relation.
    ///     let _ = SelectQuery::<MockDialect>::new().from::<User>().as_cte::<User>();
    /// }
    /// ```
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
            let _ = index;
            out.push('?');
        }
    }

    crate::table! {
        /// The `users` table.
        User => "users" {
            UserId => "id": i64,
        }
    }

    /// A query recording one projection, the shape every conversion below starts from.
    fn user_ids() -> SelectQuery<MockDialect, User, (i64,), HasProjection> {
        SelectQuery::<MockDialect>::new()
            .from::<User>()
            .select((UserId,))
    }

    #[test]
    fn building_a_query_borrows_the_clauses_it_records() {
        let query = user_ids();
        let statement = query.statement();

        // The statement view aliases the clause vectors the builder owns, so
        // validating and rendering it copies neither them nor the statements and
        // expressions they hold: a plain `build` allocates no clause vector.
        assert_eq!(statement.projections.as_ptr(), query.projections.as_ptr());
        assert_eq!(statement.ctes.as_ptr(), query.ctes.as_ptr());
        assert_eq!(statement.joins.as_ptr(), query.joins.as_ptr());
        assert_eq!(statement.order_by.as_ptr(), query.order_by.as_ptr());
        assert_eq!(
            statement.set_operations.as_ptr(),
            query.set_operations.as_ptr()
        );
    }

    #[test]
    fn building_a_query_twice_keeps_the_builder_intact() {
        let query = user_ids();

        let first = query.build().expect("valid query");
        let second = query.build().expect("valid query");

        // A borrowed build leaves the recorded clauses in place, so the same
        // statement renders again without re-assembling anything.
        assert_eq!(first, second);
        assert_eq!(first.sql, "SELECT \"id\" FROM \"users\"");
    }

    #[test]
    fn a_consuming_conversion_moves_the_recorded_clauses() {
        let query = user_ids();
        let projections = query.projections.as_ptr();
        let ctes = query.ctes.as_ptr();

        let ast = query.into_ast();

        // The conversion hands the clauses over, so a subquery, CTE, lateral join,
        // or derived table built from a consumed builder allocates no second copy
        // of the statement.
        assert_eq!(ast.projections.as_ptr(), projections);
        assert_eq!(ast.ctes.as_ptr(), ctes);
        assert_eq!(ast.projections.len(), 1);
    }

    #[test]
    fn a_borrowing_conversion_copies_the_recorded_clauses() {
        let query = user_ids();

        let ast = query.to_ast();

        // The borrowed form keeps the query usable, so the statement owns a copy of
        // every clause instead of aliasing them.
        assert_eq!(ast.projections, query.projections);
        assert_ne!(ast.projections.as_ptr(), query.projections.as_ptr());
    }

    #[test]
    fn a_consuming_exists_predicate_moves_the_nested_statement() {
        let query = user_ids();
        let projections = query.projections.as_ptr();

        let nested = match query.into_exists().into_kind() {
            ExprKind::Subquery {
                op: SubqueryOp::Exists,
                query,
                negated: false,
                ..
            } => query,
            other => panic!("expected an EXISTS predicate, got {other:?}"),
        };

        assert_eq!(nested.ast().projections.as_ptr(), projections);
    }
}
